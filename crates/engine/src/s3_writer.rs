use common::*;
use aws_sdk_s3::Client;

pub struct S3Writer {
    client: Client,
    bucket: String,
}

impl S3Writer {
    pub fn new(client: Client, bucket: String) -> Self {
        Self { client, bucket }
    }
    
    // TODO: Implement parquet writing
}