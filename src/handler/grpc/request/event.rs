// Copyright 2023 Zinc Labs Inc.
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

use opentelemetry::global;
use tonic::{Request, Response, Status};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::common::{
    infra::{config::CONFIG, metrics},
    meta::common::FileMeta,
};
use crate::handler::grpc::cluster_rpc::{event_server::Event, EmptyResponse, FileList};
use crate::service::db::file_list;

pub struct Eventer;

#[tonic::async_trait]
impl Event for Eventer {
    async fn send_file_list(
        &self,
        req: Request<FileList>,
    ) -> Result<Response<EmptyResponse>, Status> {
        let start = std::time::Instant::now();
        let parent_cx = global::get_text_map_propagator(|prop| {
            prop.extract(&super::MetadataMap(req.metadata()))
        });
        tracing::Span::current().set_parent(parent_cx);

        let req = req.get_ref();
        for file in req.items.iter() {
            if CONFIG.common.print_key_event {
                log::info!(
                    "Received event:file {}, deleted: {}",
                    file.key,
                    file.deleted
                );
            }
            if let Err(e) = file_list::progress(
                &file.key,
                FileMeta::from(file.meta.as_ref().unwrap()),
                file.deleted,
                true,
            )
            .await
            {
                // metrics
                let time = start.elapsed().as_secs_f64();
                metrics::GRPC_RESPONSE_TIME
                    .with_label_values(&["/event/send_file_list", "500", "", "", ""])
                    .observe(time);
                metrics::GRPC_INCOMING_REQUESTS
                    .with_label_values(&["/event/send_file_list", "500", "", "", ""])
                    .inc();
                return Err(Status::internal(e.to_string()));
            }
        }

        // metrics
        let time = start.elapsed().as_secs_f64();
        metrics::GRPC_RESPONSE_TIME
            .with_label_values(&["/event/send_file_list", "200", "", "", ""])
            .observe(time);
        metrics::GRPC_INCOMING_REQUESTS
            .with_label_values(&["/event/send_file_list", "200", "", "", ""])
            .inc();

        Ok(Response::new(EmptyResponse {}))
    }
}
