use crate::dashboard::{DashboardPaginationParameters, DashboardTemplate, lamports_to_sol};
use crate::state::read_state;
use crate::test_fixtures::{
    DEPOSIT_FEE, MINIMUM_DEPOSIT_AMOUNT, MINIMUM_WITHDRAWAL_AMOUNT, WITHDRAWAL_FEE, account,
    deposit_id,
    events::{
        accept_deposit, accept_withdrawal, fail_transaction, mint_deposit, quarantine_deposit,
        submit_consolidation, submit_withdrawal, succeed_transaction,
    },
    init_schnorr_master_key, init_state, init_state_with_args, ledger_canister_id,
    runtime::TestCanisterRuntime,
    signature, sol_rpc_canister_id, valid_init_args,
};
use askama::Template;
use cksol_types_internal::SolanaNetwork;

fn dashboard() -> DashboardTemplate {
    read_state(|state| {
        DashboardTemplate::from_state(
            state,
            &TestCanisterRuntime::new(),
            DashboardPaginationParameters::default(),
        )
    })
}

fn dashboard_with_pagination(pagination: DashboardPaginationParameters) -> DashboardTemplate {
    read_state(|state| {
        DashboardTemplate::from_state(state, &TestCanisterRuntime::new(), pagination)
    })
}

fn init_state_with_network(network: SolanaNetwork) {
    let mut args = valid_init_args();
    args.solana_network = network;
    init_state_with_args(args);
}

#[test]
fn should_display_metadata() {
    init_state();

    DashboardAssert::assert_that(dashboard())
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
fn should_display_empty_state() {
    init_state();

    DashboardAssert::assert_that(dashboard())
        .has_no_elements_matching("#deposits + table")
        .has_no_elements_matching("#withdrawals + table");
}

#[test]
fn should_display_minter_address_when_not_set() {
    init_state();

    DashboardAssert::assert_that(dashboard()).has_string_value(
        "#minter-address > td",
        "N/A",
        "expected N/A when minter address not set",
    );
}

#[test]
fn should_display_minter_address_with_mainnet_solscan_link() {
    init_state_with_network(SolanaNetwork::Mainnet);
    init_schnorr_master_key();

    let assert = DashboardAssert::assert_that(dashboard());
    let address = assert.text_value("#minter-address > td");
    assert
        .has_link_matching("#minter-address a", |href| {
            href == format!("https://solscan.io/account/{address}")
        })
        .has_link_matching("#solana-cluster a", |href| href == "https://solscan.io/");
}

#[test]
fn should_display_minter_address_with_devnet_solscan_link() {
    init_state_with_network(SolanaNetwork::Devnet);
    init_schnorr_master_key();

    let assert = DashboardAssert::assert_that(dashboard());
    let address = assert.text_value("#minter-address > td");
    assert
        .has_link_matching("#minter-address a", |href| {
            href == format!("https://solscan.io/account/{address}?cluster=devnet")
        })
        .has_link_matching("#solana-cluster a", |href| {
            href == "https://solscan.io/?cluster=devnet"
        });
}

#[test]
fn should_display_minted_deposits() {
    init_state();

    let deposit = deposit_id(1);
    let deposit_amount = 500_000_000;
    accept_deposit(deposit, deposit_amount);
    mint_deposit(deposit, 42);

    DashboardAssert::assert_that(dashboard())
        .has_table_row_value(
            "#deposits + table > tbody > tr:nth-child(1)",
            &[
                &deposit.signature.to_string(),
                &deposit.account.to_string(),
                &lamports_to_sol(deposit_amount),
                &lamports_to_sol(deposit_amount - DEPOSIT_FEE),
                "42",
                "Minted",
            ],
            "deposits",
        )
        .has_links_satisfying(
            |href| href.contains("solscan.io/tx/"),
            |href| href.contains(&deposit.signature.to_string()),
        );
}

#[test]
fn should_display_all_deposit_statuses() {
    init_state();

    // Accepted
    accept_deposit(deposit_id(1), 100_000_000);

    // Minted (pending consolidation)
    accept_deposit(deposit_id(2), 200_000_000);
    mint_deposit(deposit_id(2), 10);

    // Quarantined
    accept_deposit(deposit_id(3), 50_000_000);
    quarantine_deposit(deposit_id(3));

    // Consolidated (minted + consolidation submitted)
    accept_deposit(deposit_id(4), 300_000_000);
    mint_deposit(deposit_id(4), 20);
    submit_consolidation(signature(0xAA), account(0), 1, vec![20]);

    let rendered_dashboard = dashboard();
    assert_eq!(rendered_dashboard.deposits_table.current_page.len(), 4);

    let statuses: Vec<&str> = rendered_dashboard
        .deposits_table
        .current_page
        .iter()
        .map(|deposit| deposit.status)
        .collect();
    assert!(statuses.contains(&"Accepted"));
    assert!(statuses.contains(&"Quarantined"));
    assert!(statuses.contains(&"Minted"));
    assert!(statuses.contains(&"Consolidated"));
}

#[test]
fn should_not_display_pagination_for_small_tables() {
    init_state();

    let rendered = dashboard().render().unwrap();
    assert!(
        !rendered.contains("Pages:"),
        "should not show pagination when tables are empty"
    );
}

#[test]
fn should_paginate_minted_deposits_across_multiple_pages() {
    use crate::dashboard::DEFAULT_PAGE_SIZE;

    init_state();

    let total_deposits = DEFAULT_PAGE_SIZE * 2 + 1;
    let remainder = total_deposits - DEFAULT_PAGE_SIZE * 2;

    for i in 0..total_deposits {
        accept_deposit(deposit_id(i as u8), 500_000_000);
        mint_deposit(deposit_id(i as u8), i as u64);
    }

    let page1 = dashboard();
    assert_eq!(page1.deposits_table.current_page.len(), DEFAULT_PAGE_SIZE);
    assert!(page1.deposits_table.has_more_than_one_page());
    assert_eq!(page1.deposits_table.pagination.pages.len(), 3);
    assert_eq!(page1.deposits_table.pagination.current_page_index, 1);

    let rendered = page1.render().unwrap();
    assert!(
        rendered.contains("Pages:"),
        "should show pagination controls"
    );

    let page2 = dashboard_with_pagination(DashboardPaginationParameters {
        minted_deposits_start: DEFAULT_PAGE_SIZE,
        ..Default::default()
    });
    assert_eq!(page2.deposits_table.current_page.len(), DEFAULT_PAGE_SIZE);
    assert_eq!(page2.deposits_table.pagination.current_page_index, 2);

    let page3 = dashboard_with_pagination(DashboardPaginationParameters {
        minted_deposits_start: DEFAULT_PAGE_SIZE * 2,
        ..Default::default()
    });
    assert_eq!(page3.deposits_table.current_page.len(), remainder);
    assert_eq!(page3.deposits_table.pagination.current_page_index, 3);
}

// --- Withdrawal table tests ---

#[test]
fn should_display_all_withdrawal_statuses() {
    init_state();

    // Pending
    accept_withdrawal(account(1), 0, 100_000_000);

    // Sent
    accept_withdrawal(account(2), 1, 200_000_000);
    submit_withdrawal(signature(0xCC), account(0), 1, vec![1]);

    // Succeeded
    accept_withdrawal(account(3), 2, 300_000_000);
    submit_withdrawal(signature(0xDD), account(0), 2, vec![2]);
    succeed_transaction(signature(0xDD));

    // Failed
    accept_withdrawal(account(4), 3, 400_000_000);
    submit_withdrawal(signature(0xEE), account(0), 3, vec![3]);
    fail_transaction(signature(0xEE));

    let rendered_dashboard = dashboard();
    assert_eq!(rendered_dashboard.withdrawals_table.current_page.len(), 4);

    let statuses: Vec<&str> = rendered_dashboard
        .withdrawals_table
        .current_page
        .iter()
        .map(|withdrawal| withdrawal.status)
        .collect();
    assert!(statuses.contains(&"Pending"));
    assert!(statuses.contains(&"Sent"));
    assert!(statuses.contains(&"Succeeded"));
    assert!(statuses.contains(&"Failed"));
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

    fn has_table_row_value(
        &self,
        selector: &str,
        expected_values: &[&str],
        error_msg: &str,
    ) -> &Self {
        let css_selector = scraper::Selector::parse(selector).unwrap();
        let element = self.actual.select(&css_selector).next().unwrap_or_else(|| {
            panic!(
                "expected element for selector '{selector}', got none. Rendered html: {}",
                self.rendered_html
            )
        });
        let values: Vec<_> = element
            .text()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(
            values, expected_values,
            "{}. Rendered html: {}",
            error_msg, self.rendered_html
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
