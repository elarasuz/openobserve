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

use async_trait::async_trait;

use crate::common::infra::{
    config::CONFIG,
    errors::{Error, Result},
};
use crate::common::meta::{
    common::{FileKey, FileMeta},
    stream::PartitionTimeLevel,
    StreamType,
};

pub mod duckdb;
pub mod dynamo;
pub mod postgres;
pub mod sled;
pub mod sqlite;

lazy_static! {
    static ref CLIENT: Box<dyn FileList> = connect();
}

pub fn connect() -> Box<dyn FileList> {
    match CONFIG.common.file_list_storage.as_str() {
        "sled" => Box::<sled::SledFileList>::default(),
        "sqlite" => Box::<sqlite::SqliteFileList>::default(),
        "postgres" | "postgresql" => Box::<postgres::PostgresFileList>::default(),
        "dynamo" | "dynamodb" => Box::<dynamo::DynamoFileList>::default(),
        "duckdb" => Box::<duckdb::DuckDBFileList>::default(),
        _ => Box::<sqlite::SqliteFileList>::default(),
    }
}

#[async_trait]
pub trait FileList: Sync + 'static {
    async fn add(&self, file: &str, meta: &FileMeta) -> Result<()>;
    async fn remove(&self, file: &str) -> Result<()>;
    async fn batch_add(&self, files: &[FileKey]) -> Result<()>;
    async fn batch_remove(&self, files: &[String]) -> Result<()>;
    async fn get(&self, file: &str) -> Result<FileMeta>;
    async fn list(&self) -> Result<Vec<(String, FileMeta)>>;
    async fn query(
        &self,
        org_id: &str,
        stream_type: StreamType,
        stream_name: &str,
        time_level: PartitionTimeLevel,
        time_range: (i64, i64),
    ) -> Result<Vec<(String, FileMeta)>>;
    async fn contains(&self, file: &str) -> Result<bool>;
    async fn len(&self) -> usize;
    async fn is_empty(&self) -> bool;
    async fn clear(&self) -> Result<()>;
}

pub async fn init() -> Result<()> {
    // check cache dir
    std::fs::create_dir_all(&CONFIG.common.data_cache_dir)?;
    match CONFIG.common.file_list_storage.as_str() {
        "sled" => sled::init().await,
        "sqlite" => sqlite::init().await,
        "postgres" | "postgresql" => postgres::init().await,
        "dynamo" | "dynamodb" => dynamo::init().await,
        "duckdb" => duckdb::init().await,
        _ => sqlite::init().await,
    }
}

#[inline]
pub async fn add(file: &str, meta: &FileMeta) -> Result<()> {
    CLIENT.add(file, meta).await
}

#[inline]
pub async fn remove(file: &str) -> Result<()> {
    CLIENT.remove(file).await
}

#[inline]
pub async fn batch_add(files: &[FileKey]) -> Result<()> {
    CLIENT.batch_add(files).await
}

#[inline]
pub async fn batch_remove(files: &[String]) -> Result<()> {
    CLIENT.batch_remove(files).await
}

#[inline]
pub async fn get(file: &str) -> Result<FileMeta> {
    CLIENT.get(file).await
}

#[inline]
pub async fn list() -> Result<Vec<(String, FileMeta)>> {
    CLIENT.list().await
}

#[inline]
pub async fn query(
    org_id: &str,
    stream_type: StreamType,
    stream_name: &str,
    time_level: PartitionTimeLevel,
    time_range: (i64, i64),
) -> Result<Vec<(String, FileMeta)>> {
    CLIENT
        .query(org_id, stream_type, stream_name, time_level, time_range)
        .await
}

#[inline]
pub async fn contains(file: &str) -> Result<bool> {
    CLIENT.contains(file).await
}

#[inline]
pub async fn len() -> usize {
    CLIENT.len().await
}

#[inline]
pub async fn is_empty() -> bool {
    CLIENT.is_empty().await
}

#[inline]
pub async fn clear() -> Result<()> {
    CLIENT.clear().await
}

/// parse file key to get stream_key, date_key, file_name
pub fn parse_file_key_columns(key: &str) -> Result<(String, String, String)> {
    // eg: files/default/logs/olympics/2022/10/03/10/6982652937134804993_1.parquet
    let columns = key.splitn(9, '/').collect::<Vec<&str>>();
    if columns.len() < 9 {
        return Err(Error::Message(format!(
            "[file_list] Invalid file path: {}",
            key
        )));
    }
    // let _ = columns[0].to_string(); // files/
    let stream_key = format!("{}/{}/{}", columns[1], columns[2], columns[3]);
    let date_key = format!(
        "{}/{}/{}/{}",
        columns[4], columns[5], columns[6], columns[7]
    );
    let file_name = columns[8].to_string();
    Ok((stream_key, date_key, file_name))
}
