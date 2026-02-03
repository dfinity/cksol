use cksol_int_tests::Setup;
use cksol_types::{DummyRequest, DummyResponse};

#[tokio::test]
async fn should_greet() {
    let setup = Setup::new().await;

    let request = DummyRequest {
        input: "world".to_string(),
    };
    let response: DummyResponse = setup.minter().query_call("greet", (request,)).await;

    assert_eq!(
        response,
        DummyResponse {
            output: "Hello, world!".to_string()
        }
    );
}
