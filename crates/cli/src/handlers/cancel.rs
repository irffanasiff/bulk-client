use crate::commands::{CancelAllArgs, CancelArgs};
use crate::common::{submit_actions, SubmitOptions};
use bulk_client::msgs::{CancelAll, CancelOrder};
use bulk_client::transaction::Action;
use bulk_client::BulkHttpClient;

pub async fn handle_cancel(
    api: Option<&BulkHttpClient>,
    args: CancelArgs,
    submit: &SubmitOptions,
) -> eyre::Result<()> {
    submit.progress(format_args!("Cancelling order {}", args.order_id))?;

    let action = Action::Cancel(CancelOrder {
        symbol: args.symbol,
        oid: args.order_id,
        meta: Default::default(),
    });

    submit_actions(api, submit, vec![action]).await
}

pub async fn handle_cancel_all(
    api: Option<&BulkHttpClient>,
    args: CancelAllArgs,
    submit: &SubmitOptions,
) -> eyre::Result<()> {
    let symbols = match &args.instrument {
        Some(inst) => {
            submit.progress(format_args!("Cancelling all orders for {inst}"))?;
            vec![inst.clone()]
        }
        None => {
            submit.progress(format_args!("Cancelling all open orders"))?;
            vec![]
        }
    };

    let action = Action::CancelAll(CancelAll {
        symbols,
        meta: Default::default(),
    });

    submit_actions(api, submit, vec![action]).await
}
