use crate::commands::{
    AgentWalletArgs, CreateSubAccountArgs, FaucetArgs, RemoveSubAccountArgs, TransferArgs,
    UpdateLeverageArgs,
};
use crate::common::{submit_actions, SubmitOptions};
use bulk_client::msgs::subaccounts::{CreateSubAccount, RemoveSubAccount, Transfer};
use bulk_client::msgs::{AgentWalletCreation, Faucet, UpdateUserSettings};
use bulk_client::transaction::Action;
use bulk_client::BulkHttpClient;
use std::collections::HashMap;
use std::sync::Arc;

pub async fn handle_faucet(
    api: Option<&BulkHttpClient>,
    args: FaucetArgs,
    submit: &SubmitOptions,
) -> eyre::Result<()> {
    let account = submit
        .unsigned
        .as_ref()
        .map(|u| u.account)
        .or_else(|| api.and_then(|api| api.public_key()))
        .ok_or_else(|| eyre::eyre!("account required"))?;
    submit.progress(format_args!("Faucet request for account {}", account))?;

    let action = Action::Faucet(Faucet {
        user: account,
        amount: args.amount,
        meta: Default::default(),
    });

    submit_actions(api, submit, vec![action]).await
}

// ---------------------------------------------------------------------------
// Leverage
// ---------------------------------------------------------------------------

pub async fn handle_update_leverage(
    api: Option<&BulkHttpClient>,
    args: UpdateLeverageArgs,
    submit: &SubmitOptions,
) -> eyre::Result<()> {
    for (sym, lev) in &args.settings {
        submit.progress(format_args!("  {sym} → {lev}x"))?;
    }

    let max_leverage: HashMap<String, f64> = args.settings.into_iter().collect();

    let action = Action::UpdateUserSettings(UpdateUserSettings {
        max_leverage,
        meta: Default::default(),
    });

    submit_actions(api, submit, vec![action]).await
}

// ---------------------------------------------------------------------------
// Agent wallet
// ---------------------------------------------------------------------------

pub async fn handle_agent_wallet(
    api: Option<&BulkHttpClient>,
    args: AgentWalletArgs,
    submit: &SubmitOptions,
) -> eyre::Result<()> {
    let verb = if args.delete { "Removing" } else { "Adding" };
    submit.progress(format_args!("{verb} agent wallet {}", args.agent))?;

    let action = Action::AgentWalletCreation(AgentWalletCreation {
        agent: args.agent,
        delete: args.delete,
        meta: Default::default(),
    });

    submit_actions(api, submit, vec![action]).await
}

// ---------------------------------------------------------------------------
// CreateSubAccount
// ---------------------------------------------------------------------------

pub async fn handle_create_subaccount(
    api: Option<&BulkHttpClient>,
    args: CreateSubAccountArgs,
    submit: &SubmitOptions,
) -> eyre::Result<()> {
    submit.progress(format_args!(
        "Creating sub-account '{}' margin_symbol={:?} margin_amount={:?}",
        args.name, args.margin_symbol, args.margin_amount
    ))?;

    let action = Action::CreateSubAccount(CreateSubAccount {
        name: Arc::from(args.name.as_str()),
        margin_amount: args.margin_amount,
        meta: Default::default(),
    });

    submit_actions(api, submit, vec![action]).await
}

// ---------------------------------------------------------------------------
// RemoveSubAccount
// ---------------------------------------------------------------------------

pub async fn handle_remove_subaccount(
    api: Option<&BulkHttpClient>,
    args: RemoveSubAccountArgs,
    submit: &SubmitOptions,
) -> eyre::Result<()> {
    submit.progress(format_args!("Removing sub-account {}", args.pubkey))?;

    let action = Action::RemoveSubAccount(RemoveSubAccount {
        to_remove: args.pubkey,
        meta: Default::default(),
    });

    submit_actions(api, submit, vec![action]).await
}

// ---------------------------------------------------------------------------
// Transfer
// ---------------------------------------------------------------------------

pub async fn handle_transfer(
    api: Option<&BulkHttpClient>,
    args: TransferArgs,
    submit: &SubmitOptions,
) -> eyre::Result<()> {
    submit.progress(format_args!(
        "Transferring {} {} from {} → {} ({:?})",
        args.amount, args.symbol, args.from, args.to, args.kind
    ))?;

    let action = Action::Transfer(Transfer {
        kind: args.kind,
        from: args.from,
        to: args.to,
        margin_amount: args.amount,
        meta: Default::default(),
    });

    submit_actions(api, submit, vec![action]).await
}
