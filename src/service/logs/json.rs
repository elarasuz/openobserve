// Copyright 2022 Zinc Labs Inc. and Contributors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use actix_web::{http, web};
use ahash::AHashMap;
use chrono::{Duration, Utc};
use datafusion::arrow::datatypes::Schema;

use super::StreamMeta;
use crate::common::infra::{cluster, config::CONFIG, metrics};
use crate::common::meta::stream::StreamParams;
use crate::common::meta::usage::UsageType;
use crate::common::meta::{
    alert::{Alert, Trigger},
    ingestion::{IngestionResponse, StreamStatus},
    StreamType,
};
use crate::common::{flatten, json, time::parse_timestamp_micro_from_value};
use crate::service::{
    db, format_stream_name, ingestion::write_file, schema::stream_schema_exists,
    usage::report_request_usage_stats,
};

pub async fn ingest(
    org_id: &str,
    in_stream_name: &str,
    body: web::Bytes,
    thread_id: usize,
) -> Result<IngestionResponse, anyhow::Error> {
    let start = std::time::Instant::now();
    let stream_name = &format_stream_name(in_stream_name);

    println!("i am here 1");

    if !cluster::is_ingester(&cluster::LOCAL_NODE_ROLE) {
        return Err(anyhow::anyhow!("not an ingester"));
    }

    if !db::file_list::BLOCKED_ORGS.is_empty() && db::file_list::BLOCKED_ORGS.contains(&org_id) {
        return Err(anyhow::anyhow!("Quota exceeded for this organisation"));
    }

    // check if we are allowed to ingest
    if db::compact::retention::is_deleting_stream(org_id, stream_name, StreamType::Logs, None) {
        return Err(anyhow::anyhow!("stream [{stream_name}] is being deleted"));
    }

    println!("i am here 2");

    let mut min_ts =
        (Utc::now() + Duration::hours(CONFIG.limit.ingest_allowed_upto)).timestamp_micros();

    let mut runtime = crate::service::ingestion::init_functions_runtime();

    let mut stream_schema_map: AHashMap<String, Schema> = AHashMap::new();
    let mut stream_alerts_map: AHashMap<String, Vec<Alert>> = AHashMap::new();
    let mut stream_status = StreamStatus::new(stream_name);

    let mut trigger: Option<Trigger> = None;

    println!("i am here 3");

    // Start Register Transforms for stream

    let (local_trans, stream_vrl_map) = crate::service::ingestion::register_stream_transforms(
        org_id,
        StreamType::Logs,
        stream_name,
    );
    // End Register Transforms for stream

    println!("i am here 4");

    let stream_schema = stream_schema_exists(
        org_id,
        stream_name,
        StreamType::Logs,
        &mut stream_schema_map,
    )
    .await;

    println!("i am here 5");

    let mut partition_keys: Vec<String> = vec![];
    if stream_schema.has_partition_keys {
        let partition_det =
            crate::service::ingestion::get_stream_partition_keys(stream_name, &stream_schema_map)
                .await;
        partition_keys = partition_det.partition_keys;
    }

    println!("i am here 6");

    // Start get stream alerts
    let key = format!("{}/{}/{}", &org_id, StreamType::Logs, &stream_name);
    crate::service::ingestion::get_stream_alerts(key, &mut stream_alerts_map).await;
    // End get stream alert

    println!("i am here 7");

    let mut buf: AHashMap<String, Vec<String>> = AHashMap::new();
    let reader: Vec<json::Value> = json::from_slice(&body)?;

    // debug
    let mut iii = 0;
    for item in reader.iter() {
        //JSON Flattening
        let mut value = flatten::flatten(item)?;

        if !local_trans.is_empty() {
            value = crate::service::ingestion::apply_stream_transform(
                &local_trans,
                &value,
                &stream_vrl_map,
                stream_name,
                &mut runtime,
            )?;
        }

        if value.is_null() || !value.is_object() {
            stream_status.status.failed += 1; // transform failed or dropped
            continue;
        }
        // End row based transform

        if iii == 0 {
            println!("i am here 8");
            iii = 1;
        }

        // get json object
        let local_val = value.as_object_mut().unwrap();

        // handle timestamp
        let timestamp = match local_val.get(&CONFIG.common.column_timestamp) {
            Some(v) => match parse_timestamp_micro_from_value(v) {
                Ok(t) => t,
                Err(e) => {
                    stream_status.status.failed += 1;
                    stream_status.status.error = e.to_string();
                    continue;
                }
            },
            None => Utc::now().timestamp_micros(),
        };
        // check ingestion time
        let earlest_time = Utc::now() + Duration::hours(0 - CONFIG.limit.ingest_allowed_upto);
        if timestamp < earlest_time.timestamp_micros() {
            stream_status.status.failed += 1; // to old data, just discard
            stream_status.status.error = super::get_upto_discard_error();
            continue;
        }
        if timestamp < min_ts {
            min_ts = timestamp;
        }
        local_val.insert(
            CONFIG.common.column_timestamp.clone(),
            json::Value::Number(timestamp.into()),
        );

        if iii == 0 {
            println!("i am here 9");
            iii = 1;
        }

        let local_trigger = super::add_valid_record(
            StreamMeta {
                org_id: org_id.to_string(),
                stream_name: stream_name.to_string(),
                partition_keys: partition_keys.clone(),
                stream_alerts_map: stream_alerts_map.clone(),
            },
            &mut stream_schema_map,
            &mut stream_status.status,
            &mut buf,
            local_val,
        )
        .await;

        if local_trigger.is_some() {
            trigger = Some(local_trigger.unwrap());
        }

        if iii == 0 {
            println!("i am here 10");
            iii = 1;
        }
    }

    // write to file
    let mut stream_file_name = "".to_string();
    let mut req_stats = write_file(
        buf,
        thread_id,
        &mut stream_file_name,
        StreamParams {
            org_id,
            stream_name,
            stream_type: StreamType::Logs,
        },
        None,
    );

    if stream_file_name.is_empty() {
        return Ok(IngestionResponse::new(
            http::StatusCode::OK.into(),
            vec![stream_status],
        ));
    }

    println!("i am here 11");

    // only one trigger per request, as it updates etcd
    super::evaluate_trigger(trigger, stream_alerts_map).await;

    println!("i am here 12");

    let time = start.elapsed().as_secs_f64();
    metrics::HTTP_RESPONSE_TIME
        .with_label_values(&[
            "/api/org/ingest/logs/_json",
            "200",
            org_id,
            stream_name,
            StreamType::Logs.to_string().as_str(),
        ])
        .observe(time);
    metrics::HTTP_INCOMING_REQUESTS
        .with_label_values(&[
            "/api/org/ingest/logs/_json",
            "200",
            org_id,
            stream_name,
            StreamType::Logs.to_string().as_str(),
        ])
        .inc();

    println!("i am here 13");

    req_stats.response_time = start.elapsed().as_secs_f64();
    //metric + data usage
    report_request_usage_stats(
        req_stats,
        org_id,
        stream_name,
        StreamType::Logs,
        UsageType::Json,
        local_trans.len() as u16,
    )
    .await;

    println!("i am here 14");

    Ok(IngestionResponse::new(
        http::StatusCode::OK.into(),
        vec![stream_status],
    ))
}
