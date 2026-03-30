use crate::dashboard::{DashboardTemplate, lamports_to_sol};
use crate::state::{SchnorrPublicKey, State};
use crate::test_fixtures::{
    DEPOSIT_FEE, MINIMUM_DEPOSIT_AMOUNT, MINIMUM_WITHDRAWAL_AMOUNT, WITHDRAWAL_FEE,
    ledger_canister_id, sol_rpc_canister_id, valid_init_args,
};
use askama::Template;
use cksol_types_internal::SolanaNetwork;
use ic_ed25519::{PocketIcMasterPublicKeyId, PublicKey};

fn initial_state() -> State {
    State::try_from(valid_init_args()).expect("valid init args")
}

fn state_with_network(network: SolanaNetwork) -> State {
    let mut args = valid_init_args();
    args.solana_network = network;
    State::try_from(args).expect("valid init args")
}

fn state_with_minter_key(network: SolanaNetwork) -> State {
    let mut state = state_with_network(network);
    state.set_once_minter_public_key(SchnorrPublicKey {
        public_key: PublicKey::pocketic_key(PocketIcMasterPublicKeyId::Key1),
        chain_code: [1; 32],
    });
    state
}

fn initial_dashboard() -> DashboardTemplate {
    DashboardTemplate::from_state(&initial_state())
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
            &lamports_to_sol(DEPOSIT_FEE),
            "wrong deposit fee",
        )
        .has_string_value(
            "#withdrawal-fee > td",
            &lamports_to_sol(WITHDRAWAL_FEE),
            "wrong withdrawal fee",
        )
        .has_string_value(
            "#minimum-deposit-amount > td",
            &lamports_to_sol(MINIMUM_DEPOSIT_AMOUNT),
            "wrong minimum deposit amount",
        )
        .has_string_value(
            "#minimum-withdrawal-amount > td",
            &lamports_to_sol(MINIMUM_WITHDRAWAL_AMOUNT),
            "wrong minimum withdrawal amount",
        );
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
fn should_display_minter_address_with_mainnet_solscan_link() {
    let state = state_with_minter_key(SolanaNetwork::Mainnet);
    let dashboard = DashboardTemplate::from_state(&state);

    let assert = DashboardAssert::assert_that(dashboard);
    let address = assert.text_value("#minter-address > td");
    assert
        .has_link_matching("#minter-address a", |href| {
            href == format!("https://solscan.io/account/{address}")
        })
        .has_link_matching("#solana-cluster a", |href| href == "https://solscan.io/");
}

#[test]
fn should_display_minter_address_with_devnet_solscan_link() {
    let state = state_with_minter_key(SolanaNetwork::Devnet);
    let dashboard = DashboardTemplate::from_state(&state);

    let assert = DashboardAssert::assert_that(dashboard);
    let address = assert.text_value("#minter-address > td");
    assert
        .has_link_matching("#minter-address a", |href| {
            href == format!("https://solscan.io/account/{address}?cluster=devnet")
        })
        .has_link_matching("#solana-cluster a", |href| {
            href == "https://solscan.io/?cluster=devnet"
        });
}

// --- Assertion helpers ---

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

    fn text_value(&self, selector: &str) -> String {
        let css_selector = scraper::Selector::parse(selector).unwrap();
        let element = self.actual.select(&css_selector).next().unwrap_or_else(|| {
            panic!(
                "expected element for selector '{selector}', got none. Rendered html: {}",
                self.rendered_html
            )
        });
        element.text().collect::<String>().trim().to_string()
    }

    fn has_link_matching(&self, selector: &str, predicate: impl Fn(&str) -> bool) -> &Self {
        let css_selector = scraper::Selector::parse(selector).unwrap();
        let element = self.actual.select(&css_selector).next().unwrap_or_else(|| {
            panic!(
                "expected element for selector '{selector}', got none. Rendered html: {}",
                self.rendered_html
            )
        });
        let href = element.value().attr("href").unwrap_or_else(|| {
            panic!(
                "expected href for selector '{selector}'. Rendered html: {}",
                self.rendered_html
            )
        });
        assert!(
            predicate(href),
            "Link href '{href}' did not match predicate for selector '{selector}'. Rendered html: {}",
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
}
