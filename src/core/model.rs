use crate::error::AgentError;
use openresponses_rust::{Client, CreateResponseBody, ResponseResource};
use std::sync::Arc;

#[derive(Clone)]
pub struct ModelClient {
    client: Arc<Client>,
}

impl ModelClient {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            client: Arc::new(Client::with_base_url(api_key.into(), base_url.into())),
        }
    }

    pub async fn create_response(
        &self,
        request: CreateResponseBody,
    ) -> Result<ResponseResource, AgentError> {
        self.client
            .create_response(request)
            .await
            .map_err(AgentError::from)
    }
}
