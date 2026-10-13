use cksol_int_tests::CkSolMinter;
use cksol_types_internal::{
    UpgradeArgs,
    event::{Event, EventType},
};
use pocket_ic::PocketIcBuilder;

const STAGING_MINTER_ADDRESS: &str = "Br8eRkeya8hy3sHCYqtWqGeNNZ349aPSUKGVYFWKKer1";

#[tokio::test]
async fn should_load_staging_snapshot_and_upgrade_to_current_wasm() {
    let env = PocketIcBuilder::new()
        .with_fiduciary_subnet()
        .build_async()
        .await;
    let minter = CkSolMinter::load_from_snapshot(&env).await;

    let state_before = minter.state().await;
    assert_eq!(
        state_before.info.minter_address.as_deref(),
        Some(STAGING_MINTER_ADDRESS)
    );
    assert!(
        !state_before.events.is_empty(),
        "the staging snapshot should contain at least one event"
    );

    let upgrade_args = UpgradeArgs::default();
    minter.stop().await;
    minter
        .upgrade(upgrade_args.clone())
        .await
        .expect("failed to upgrade the staging snapshot to the current wasm");
    minter.start().await;

    let state_after = minter.state().await;
    assert_eq!(state_after.info, state_before.info);
    let mut expected_events = payloads(&state_before.events);
    expected_events.push(EventType::Upgrade(upgrade_args));
    assert_eq!(payloads(&state_after.events), expected_events);

    env.drop().await;
}

fn payloads(events: &[Event]) -> Vec<EventType> {
    events.iter().map(|event| event.payload.clone()).collect()
}
