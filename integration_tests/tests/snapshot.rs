use assert_matches::assert_matches;
use cksol_int_tests::CkSolMinter;
use cksol_types::MinterInfo;
use cksol_types_internal::{
    UpgradeArgs,
    event::{DepositOrigin, Event, EventType},
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
    assert_eq!(
        state_after.info,
        MinterInfo {
            collected_automatic_deposit_fees: Some(0),
            ..state_before.info.clone()
        }
    );
    let (replayed_events, new_events) = state_after.events.split_at(state_before.events.len());
    let expected_events: Vec<_> = state_before
        .events
        .into_iter()
        .map(with_recorded_origin)
        .collect();
    assert_eq!(replayed_events, expected_events);
    assert_matches!(
        new_events,
        [Event { payload: EventType::Upgrade(args), .. }] if *args == upgrade_args
    );

    env.drop().await;
}

/// The event as reported by the current wasm: a `QueuedDeposit` recorded without an origin
/// predates the automatic flow, so it reports the deposit as queued by `deposit_sol`.
fn with_recorded_origin(mut event: Event) -> Event {
    if let EventType::QueuedDeposit { origin, .. } = &mut event.payload {
        origin.get_or_insert(DepositOrigin::Manual);
    }
    event
}
