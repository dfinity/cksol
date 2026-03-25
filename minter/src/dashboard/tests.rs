use crate::dashboard::{DashboardPaginationParameters, DashboardTemplate};
use crate::state::State;
use crate::test_fixtures::{
    DEPOSIT_FEE, MINIMUM_DEPOSIT_AMOUNT, MINIMUM_WITHDRAWAL_AMOUNT, WITHDRAWAL_FEE,
    ledger_canister_id, sol_rpc_canister_id, valid_init_args,
};
use askama::Template;

fn initial_state() -> State {
    State::try_from(valid_init_args()).expect("valid init args")
}

fn initial_dashboard() -> DashboardTemplate {
    DashboardTemplate::from_state(&initial_state(), DashboardPaginationParameters::default())
}

#[test]
fn should_display_metadata() {
    let dashboard = initial_dashboard();

    DashboardAssert::assert_that(dashboard)
        .has_string_value(
            "#ledger-canister-id > td",
            &ledger_canister_id().to_string(),
            "wrong ledger canister ID",
        )
        .has_string_value(
            "#sol-rpc-canister-id > td",
            &sol_rpc_canister_id().to_string(),
            "wrong sol rpc canister ID",
        )
        .has_string_value(
            "#deposit-fee > td",
            &DEPOSIT_FEE.to_string(),
            "wrong deposit fee",
        )
        .has_string_value(
            "#withdrawal-fee > td",
            &WITHDRAWAL_FEE.to_string(),
            "wrong withdrawal fee",
        )
        .has_string_value(
            "#minimum-deposit-amount > td",
            &MINIMUM_DEPOSIT_AMOUNT.to_string(),
            "wrong minimum deposit amount",
        )
        .has_string_value(
            "#minimum-withdrawal-amount > td",
            &MINIMUM_WITHDRAWAL_AMOUNT.to_string(),
            "wrong minimum withdrawal amount",
        );
}

#[test]
fn should_display_zero_counts_initially() {
    let dashboard = initial_dashboard();

    DashboardAssert::assert_that(dashboard)
        .has_no_elements_matching("#deposits-to-consolidate + table")
        .has_no_elements_matching("#pending-withdrawal-requests + table")
        .has_no_elements_matching("#sent-withdrawal-requests + table")
        .has_no_elements_matching("#submitted-transactions + table");
}

#[test]
fn should_display_minter_address_when_not_set() {
    let dashboard = initial_dashboard();

    DashboardAssert::assert_that(dashboard).has_string_value(
        "#minter-address > td",
        "N/A",
        "expected N/A when minter address not set",
    );
}

#[test]
fn should_display_solscan_links_for_submitted_transactions() {
    use crate::state::audit::process_event;
    use crate::state::event::EventType;
    use crate::test_fixtures::runtime::TestCanisterRuntime;
    use solana_hash::Hash;
    use solana_message::Message;
    use solana_signature::Signature;

    let mut state = initial_state();
    let signature = Signature::from([0x01; 64]);
    let message = Message::new_with_blockhash(
        &[],
        Some(&solana_address::Address::from([0xAA; 32])),
        &Hash::default(),
    );
    let runtime = TestCanisterRuntime::new().with_increasing_time();
    process_event(
        &mut state,
        EventType::SubmittedTransaction {
            signature,
            transaction: message,
            signers: vec![],
            slot: 42,
        },
        &runtime,
    );

    let dashboard = DashboardTemplate::from_state(&state, DashboardPaginationParameters::default());

    let assert = DashboardAssert::assert_that(dashboard);
    assert.has_links_satisfying(
        |href| href.contains("solscan.io/tx/"),
        |href| href.contains(&signature.to_string()),
    );
}

#[test]
fn should_not_display_pagination_for_small_tables() {
    let dashboard = initial_dashboard();

    let rendered = dashboard.render().unwrap();
    assert!(
        !rendered.contains("Pages:"),
        "should not show pagination when tables are empty"
    );
}

struct DashboardAssert {
    rendered_html: String,
    actual: scraper::Html,
}

impl DashboardAssert {
    fn assert_that(dashboard: DashboardTemplate) -> Self {
        let rendered_html = dashboard.render().unwrap();
        Self {
            actual: scraper::Html::parse_document(&rendered_html),
            rendered_html,
        }
    }

    fn has_no_elements_matching(&self, selector: &str) -> &Self {
        let selector = scraper::Selector::parse(selector).unwrap();
        assert!(
            self.actual.select(&selector).next().is_none(),
            "expected no elements matching '{selector:?}', but found some. Rendered html: {}",
            self.rendered_html
        );
        self
    }

    fn has_string_value(&self, selector: &str, expected_value: &str, error_msg: &str) -> &Self {
        let css_selector = scraper::Selector::parse(selector).unwrap();
        let element = self.actual.select(&css_selector).next().unwrap_or_else(|| {
            panic!(
                "expected element for selector '{selector}', got none. Rendered html: {}",
                self.rendered_html
            )
        });
        let string_value = element.text().collect::<String>();
        assert_eq!(
            string_value.trim(),
            expected_value,
            "{}. Rendered html: {}",
            error_msg,
            self.rendered_html
        );
        self
    }

    fn has_links_satisfying<F: Fn(&str) -> bool, P: Fn(&str) -> bool>(
        &self,
        filter: F,
        predicate: P,
    ) -> &Self {
        let selector = scraper::Selector::parse("a").unwrap();
        let mut found = false;
        for link in self.actual.select(&selector) {
            let href = link.value().attr("href").expect("href not found");
            if filter(href) {
                found = true;
                assert!(
                    predicate(href),
                    "Link '{href}' does not satisfy predicate. Rendered html: {}",
                    self.rendered_html
                );
            }
        }
        assert!(
            found,
            "no links matched the filter. Rendered html: {}",
            self.rendered_html
        );
        self
    }
}
