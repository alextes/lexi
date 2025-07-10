use anyhow::{anyhow, Result};
use mockall::automock;
use reqwest::{Client, StatusCode};
use tracing::{error, info, instrument};

#[allow(async_fn_in_trait)]
#[automock]
pub trait BeaconNode {
    async fn slot_status(&self, slot: u64) -> Result<StatusCode>;
}

#[derive(Clone)]
pub struct BeaconNodeHttp {
    client: Client,
    url: String,
}

impl BeaconNodeHttp {
    pub fn new(url: String) -> Self {
        Self {
            client: Client::new(),
            url,
        }
    }
}

impl BeaconNode for BeaconNodeHttp {
    #[instrument(skip(self))]
    async fn slot_status(&self, slot: u64) -> Result<StatusCode> {
        let request_url = format!("{}/eth/v1/beacon/headers/{slot}", self.url);
        info!(url = %request_url, slot = %slot, "fetching beacon header status for slot");

        match self.client.get(&request_url).send().await {
            Ok(response) => {
                info!(slot = %slot, status = %response.status(), "received status from beacon node");
                Ok(response.status())
            }
            Err(e) => {
                error!(slot = %slot, error = %e, "failed to send request to beacon node");
                Err(anyhow!(e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server as MockitoServer};
    use reqwest::StatusCode;

    #[tokio::test]
    async fn test_beacon_node_http_slot_ok() {
        let slot_number = 11805092;
        let mut server = MockitoServer::new_async().await;

        let mock_path = format!("/eth/v1/beacon/headers/{slot_number}");
        let mock = server
            .mock("get", Matcher::Exact(mock_path.clone()))
            .with_status(200)
            .create_async()
            .await;

        let beacon_node = BeaconNodeHttp::new(server.url());
        let result = beacon_node.slot_status(slot_number).await;

        mock.assert_async().await;
        match result {
            Ok(status_code) => assert_eq!(status_code, StatusCode::OK),
            Err(e) => panic!("expected Ok(StatusCode::OK), got Err({e:?})"),
        }
    }

    #[tokio::test]
    async fn test_beacon_node_http_slot_not_found() {
        let slot_number = 11805091;
        let mut server = MockitoServer::new_async().await;

        let mock_path = format!("/eth/v1/beacon/headers/{slot_number}");
        let mock = server
            .mock("get", Matcher::Exact(mock_path.clone()))
            .with_status(404)
            .create_async()
            .await;

        let beacon_node = BeaconNodeHttp::new(server.url());
        let result = beacon_node.slot_status(slot_number).await;

        mock.assert_async().await;
        match result {
            Ok(status_code) => assert_eq!(status_code, StatusCode::NOT_FOUND),
            Err(e) => panic!("expected Ok(StatusCode::NOT_FOUND), got Err({e:?})"),
        }
    }

    #[tokio::test]
    async fn test_beacon_node_http_slot_server_error() {
        let slot_number = 123456;
        let mut server = MockitoServer::new_async().await;

        let mock_path = format!("/eth/v1/beacon/headers/{slot_number}");
        let mock = server
            .mock("get", Matcher::Exact(mock_path.clone()))
            .with_status(500)
            .with_body("internal server error from mock")
            .create_async()
            .await;

        let beacon_node = BeaconNodeHttp::new(server.url());
        let result = beacon_node.slot_status(slot_number).await;

        mock.assert_async().await;
        match result {
            Ok(status_code) => assert_eq!(status_code, StatusCode::INTERNAL_SERVER_ERROR),
            Err(e) => panic!("expected Ok(StatusCode::INTERNAL_SERVER_ERROR), got Err({e:?})"),
        }
    }

    #[tokio::test]
    async fn test_beacon_node_http_request_failed() {
        let slot_number = 789101;
        // use an invalid port to ensure a connection refused error
        let beacon_node = BeaconNodeHttp::new("http://127.0.0.1:3".to_string());

        let result = beacon_node.slot_status(slot_number).await;

        assert!(result.is_err(), "expected an error, but got ok");

        // check that the underlying error is a reqwest connection error
        let is_expected_error_type = result
            .as_ref() // borrow to check the error type
            .err()
            .and_then(|e| e.downcast_ref::<reqwest::Error>())
            .is_some_and(|reqwest_err| reqwest_err.is_connect());

        assert!(
            is_expected_error_type,
            "error was not a reqwest connection error as expected. got: {:?}",
            result.err().unwrap() // safe to unwrap due to the earlier assert!(result.is_err())
        );
    }
}
