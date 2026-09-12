use bulk_client::msgs::MultisigPropose;
use bulk_client::msgs::Response;
use bulk_client::parts::make_nonce;
use bulk_client::transaction::{
    Action, ActionMeta, ClearSignMessage, SignatureDomain, Transaction,
};
use bulk_client::BulkHttpClient;
use solana_pubkey::Pubkey;
use std::fmt::Write as _;
use std::str::FromStr;

const ADMIN_MULTISIG: &str = "ADM1N11111111111111111111111111111111111113D";
const FEE_ADMIN_MULTISIG: &str = "FEEADM1N11111111111111111111111111111111113F";

#[derive(Clone, Debug)]
pub struct UnsignedOptions {
    pub account: Pubkey,
    pub signer: Pubkey,
    pub nonce: Option<u64>,
    pub signature_domain: SignatureDomain,
}

#[derive(Clone, Debug)]
pub struct SubmitOptions {
    pub preview: bool,
    pub auto_yes: bool,
    pub unsigned: Option<UnsignedOptions>,
}

impl SubmitOptions {
    pub fn progress(&self, message: std::fmt::Arguments<'_>) -> std::io::Result<()> {
        use std::io::Write;
        if self.unsigned.is_some() {
            writeln!(std::io::stderr().lock(), "{message}")
        } else {
            writeln!(std::io::stdout().lock(), "{message}")
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct UnsignedTransaction<'a> {
    version: u8,
    signature_domain: &'a str,
    account: String,
    signer: String,
    nonce: String,
    actions: &'a [Action],
    signing_payload: SigningPayload,
}

#[derive(serde::Serialize)]
struct SigningPayload {
    mode: &'static str,
    encoding: &'static str,
    data: String,
}

// Keep validation local to unsigned export: do not change upstream signing bytes.
fn validate_unsigned_actions(actions: &[Action]) -> eyre::Result<()> {
    fn positive(value: f64) -> eyre::Result<()> {
        eyre::ensure!(
            value.is_finite() && value > 0.0,
            "unsigned numeric values must be finite and positive"
        );
        Ok(())
    }
    fn fixed(value: f64) -> eyre::Result<()> {
        positive(value)?;
        // Mirrors the private SCALE in bulk_client::msgs::fixed_point.
        let scaled = (value * 1e8).round();
        eyre::ensure!(
            scaled >= 1.0 && scaled < u64::MAX as f64,
            "unsigned fixed-point value is outside the representable range"
        );
        Ok(())
    }
    for action in actions {
        match action {
            Action::LimitOrder(a) => {
                fixed(a.size)?;
                fixed(a.price)?;
            }
            Action::MarketOrder(a) => {
                fixed(a.size)?;
                if let Some(v) = a.slippage {
                    eyre::ensure!(
                        v.is_finite() && v >= 0.0 && (v * 1e8).round() < u64::MAX as f64,
                        "invalid unsigned slippage"
                    );
                }
            }
            Action::ModifyOrder(a) => fixed(a.amount)?,
            Action::Stop(a) | Action::TakeProfit(a) => {
                fixed(a.size)?;
                fixed(a.threshold)?;
                if let Some(v) = a.limit {
                    fixed(v)?;
                }
            }
            Action::Range(a) => {
                fixed(a.size)?;
                fixed(a.collar_min)?;
                fixed(a.collar_max)?;
                if let Some(v) = a.limit_min {
                    fixed(v)?;
                }
                if let Some(v) = a.limit_max {
                    fixed(v)?;
                }
            }
            Action::Trailing(a) => {
                fixed(a.size)?;
                if let Some(v) = a.limit {
                    fixed(v)?;
                }
            }
            Action::UpdateUserSettings(a) => {
                eyre::ensure!(a.max_leverage.len() <= 1,
                    "unsigned export supports at most one leverage market: upstream map encoding is not deterministic");
                for v in a.max_leverage.values() {
                    positive(*v)?;
                }
            }
            Action::Faucet(a) => {
                if let Some(v) = a.amount {
                    positive(v)?;
                }
            }
            Action::CreateSubAccount(a) => {
                if let Some(v) = a.margin_amount {
                    positive(v)?;
                }
            }
            Action::Transfer(a) => positive(a.margin_amount)?,
            Action::MultisigPropose(a) => validate_unsigned_actions(&a.actions)?,
            Action::Cancel(_)
            | Action::CancelAll(_)
            | Action::AgentWalletCreation(_)
            | Action::RemoveSubAccount(_)
            | Action::CreateMultisig(_)
            | Action::UpdateMultisigPolicy(_)
            | Action::MultisigApprove(_)
            | Action::MultisigReject(_)
            | Action::MultisigCancel(_)
            | Action::MultisigExecute(_) => {}
            _ => eyre::bail!("this action does not yet support validated unsigned export"),
        }
    }
    Ok(())
}

pub async fn submit_actions(
    api: Option<&BulkHttpClient>,
    options: &SubmitOptions,
    actions: Vec<Action>,
) -> eyre::Result<()> {
    let actions = wrap_admin_actions(actions);
    if let Some(unsigned) = &options.unsigned {
        validate_unsigned_actions(&actions)?;
        let nonce = unsigned.nonce.unwrap_or_else(make_nonce);
        let bytes = Transaction::raw_signable_bytes(
            unsigned.signature_domain,
            unsigned.account,
            nonce,
            &actions,
        )?;
        let mut hex = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            write!(&mut hex, "{byte:02x}")?;
        }
        let document = UnsignedTransaction {
            version: 1,
            signature_domain: unsigned.signature_domain.as_str(),
            account: unsigned.account.to_string(),
            signer: unsigned.signer.to_string(),
            nonce: nonce.to_string(),
            actions: &actions,
            signing_payload: SigningPayload {
                mode: "raw",
                encoding: "hex",
                data: hex,
            },
        };
        // Finish serialization before writing so validation failures cannot emit partial JSON.
        let json = serde_json::to_vec_pretty(&document)?;
        use std::io::Write;
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(&json)?;
        stdout.write_all(b"\n")?;
        return Ok(());
    }
    let nonce = make_nonce();
    let api = api.ok_or_else(|| eyre::eyre!("signed client required"))?;
    let cfg = api.config();
    let signer = cfg
        .signer
        .as_ref()
        .ok_or_else(|| eyre::eyre!("signer required"))?;
    let account = signer.public_key();

    if options.preview {
        let preview = ClearSignMessage::canonical_message(
            cfg.signature_domain
                .ok_or_else(|| eyre::eyre!("signature domain required"))?,
            account,
            nonce,
            &actions,
        )?;
        eprintln!("--- transaction preview ---");
        eprint!("{}", preview);
        if !options.auto_yes {
            use std::io::{self, Write};
            eprint!("Submit? [y/N]: ");
            io::stderr().flush()?;
            let mut buf = String::new();
            io::stdin().read_line(&mut buf)?;
            if !matches!(buf.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                return Err(eyre::eyre!("transaction rejected by user"));
            }
        }
    }

    let action_debug = proposed_action_debug(&actions);
    let results = api.place_tx(actions, None, Some(nonce)).await?;
    eprint!("{}", format_results(&results, &action_debug));
    Ok(())
}

fn format_results(results: &[Response], action_debug: &[String]) -> String {
    if let Some(created) = results
        .iter()
        .find(|response| response.status == "proposalCreated")
    {
        return format_proposal_created(created, action_debug);
    }

    let approval = results
        .iter()
        .find(|response| response.status == "proposalApproved");
    let proposal_outcome = results.iter().rev().find(|response| {
        matches!(
            response.status.as_str(),
            "proposalFailed"
                | "proposalExecuted"
                | "proposalReadyForExecution"
                | "proposalRejected"
        )
    });

    if approval.is_some() || proposal_outcome.is_some() {
        return format_approval_result(approval, proposal_outcome);
    }

    let mut output = String::from("\nStatus\n");
    output.push_str("────────────────────────────────────────\n");
    if results.is_empty() {
        output.push_str("  No response statuses returned\n");
        return output;
    }
    for response in results {
        let _ = writeln!(output, "  {}", humanize_status(&response.status));
        if let Some(message) = &response.message {
            let _ = writeln!(output, "    Message: {message}");
        }
    }
    output
}

fn format_proposal_created(created: &Response, action_debug: &[String]) -> String {
    let mut output = String::from("\nProposal Created\n");
    output.push_str("────────────────────────────────────────\n");
    write_json_field(&mut output, "Proposal", &created.raw, "proposalId");
    write_json_field(&mut output, "Required signers", &created.raw, "threshold");
    output.push_str("\nActions\n");
    output.push_str("────────────────────────────────────────\n");
    if action_debug.is_empty() {
        output.push_str("  No action details available\n");
    } else {
        for (index, action) in action_debug.iter().enumerate() {
            let _ = writeln!(output, "  [{index}] {action}");
        }
    }
    output
}

fn proposed_action_debug(actions: &[Action]) -> Vec<String> {
    actions
        .iter()
        .flat_map(|action| match action {
            Action::MultisigPropose(proposal) => proposal
                .actions
                .iter()
                .map(|nested| format!("{nested:?}"))
                .collect(),
            ordinary => vec![format!("{ordinary:?}")],
        })
        .collect()
}

fn format_approval_result(approval: Option<&Response>, outcome: Option<&Response>) -> String {
    let details = approval.or(outcome).expect("approval result exists");
    let mut output = String::from("\nApprovals\n");
    output.push_str("────────────────────────────────────────\n");
    write_json_field(&mut output, "Proposal", &details.raw, "proposalId");
    write_json_field(&mut output, "Multisig", &details.raw, "multisig");
    let approvals = details
        .raw
        .get("approvals")
        .and_then(|value| value.as_u64());
    let threshold = details
        .raw
        .get("threshold")
        .and_then(|value| value.as_u64());
    if let (Some(approvals), Some(threshold)) = (approvals, threshold) {
        let _ = writeln!(output, "  Progress: {approvals} / {threshold} approvals");
    }
    write_json_field(&mut output, "Rejections", &details.raw, "rejections");
    let rejected = outcome
        .or(approval)
        .is_some_and(|response| response.status == "proposalRejected");
    let signer_label = if rejected { "Signer" } else { "Approved by" };
    write_json_field(&mut output, signer_label, &details.raw, "signer");

    output.push_str("\nOutcome\n");
    output.push_str("────────────────────────────────────────\n");
    let final_response = outcome.or(approval).expect("approval result exists");
    let _ = writeln!(
        output,
        "  Status: {}",
        humanize_status(&final_response.status)
    );
    if let Some(message) = &final_response.message {
        let _ = writeln!(output, "  Error: {message}");
    } else if final_response.status == "proposalReadyForExecution" {
        write_json_field(
            &mut output,
            "Execute after",
            &final_response.raw,
            "executeAfter",
        );
    }
    output
}

fn write_json_field(output: &mut String, label: &str, body: &serde_json::Value, field: &str) {
    let Some(value) = body.get(field) else {
        return;
    };
    if value.is_null() {
        return;
    }
    let display = value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string());
    let _ = writeln!(output, "  {label}: {display}");
}

fn humanize_status(status: &str) -> String {
    let mut output = String::with_capacity(status.len() + 4);
    for (index, character) in status.chars().enumerate() {
        if index > 0 && character.is_ascii_uppercase() {
            output.push(' ');
        }
        if index == 0 {
            output.extend(character.to_uppercase());
        } else {
            output.push(character.to_ascii_lowercase());
        }
    }
    output
}

/// Wraps protected CLI actions in proposals to the protocol administrative multisig.
///
/// - Wraps every protected action in its own single-action proposal.
/// - Wraps multisig policy updates in a proposal to the target multisig.
/// - Preserves the original ordering of administrative and ordinary actions.
/// - Leaves existing multisig proposals and non-admin actions unchanged.
fn wrap_admin_actions(actions: Vec<Action>) -> Vec<Action> {
    let admin_multisig = Pubkey::from_str(ADMIN_MULTISIG).expect("valid admin multisig pubkey");
    let fee_admin_multisig =
        Pubkey::from_str(FEE_ADMIN_MULTISIG).expect("valid fee admin multisig pubkey");
    actions
        .into_iter()
        .map(|action| {
            let multisig = if let Action::UpdateMultisigPolicy(update) = &action {
                Some(update.multisig)
            } else if action.is_fee_admin_multisig_action() {
                Some(fee_admin_multisig)
            } else if action.is_admin_multisig_action() {
                Some(admin_multisig)
            } else {
                None
            };
            if let Some(multisig) = multisig {
                Action::MultisigPropose(MultisigPropose {
                    multisig,
                    actions: vec![action],
                    proposal_lifetime_secs: None,
                    meta: ActionMeta::default(),
                })
            } else {
                action
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bulk_client::msgs::{AddMarket, CancelAll, ConfigMakerRebateTier, OpaqueAction, UserAdmin};

    #[test]
    fn wraps_each_admin_action_once() {
        let wrapped = wrap_admin_actions(vec![
            Action::AddMarket(AddMarket {
                symbol: "BTC-USD".into(),
                meta: ActionMeta::default(),
            }),
            Action::AddMarket(AddMarket {
                symbol: "ETH-USD".into(),
                meta: ActionMeta::default(),
            }),
        ]);

        let [Action::MultisigPropose(first), Action::MultisigPropose(second)] = wrapped.as_slice()
        else {
            panic!("each admin action must be wrapped separately");
        };
        let expected_multisig = Pubkey::from_str(ADMIN_MULTISIG).unwrap();
        assert_eq!(first.multisig, expected_multisig);
        assert_eq!(second.multisig, expected_multisig);
        assert_eq!(first.actions.len(), 1);
        assert_eq!(second.actions.len(), 1);
        assert_eq!(first.proposal_lifetime_secs, None);
        assert_eq!(second.proposal_lifetime_secs, None);
        assert!(matches!(first.actions[0], Action::AddMarket(_)));
        assert!(matches!(second.actions[0], Action::AddMarket(_)));

        let wrapped_again = wrap_admin_actions(wrapped);
        assert!(matches!(
            wrapped_again.as_slice(),
            [Action::MultisigPropose(_), Action::MultisigPropose(_)]
        ));
    }

    #[test]
    fn leaves_ordinary_actions_unwrapped() {
        let wrapped = wrap_admin_actions(vec![Action::CancelAll(CancelAll {
            symbols: Vec::new(),
            meta: ActionMeta::default(),
        })]);

        assert!(matches!(wrapped.as_slice(), [Action::CancelAll(_)]));
    }

    #[test]
    fn wraps_fee_actions_with_fee_admin_multisig() {
        let wrapped = wrap_admin_actions(vec![
            Action::ConfigFeePolicy(OpaqueAction {
                payload: vec![1, 2, 3],
                meta: ActionMeta::default(),
            }),
            Action::ConfigMakerRebateTier(ConfigMakerRebateTier {
                instrument: "BTC-USD".into(),
                maker: Pubkey::new_unique(),
                minimum_tier: Some(1),
                expires_slot: None,
                meta: ActionMeta::default(),
            }),
        ]);

        let expected = Pubkey::from_str(FEE_ADMIN_MULTISIG).unwrap();
        assert!(!expected.is_on_curve());
        assert_ne!(expected.to_bytes()[31] & 0x80, 0);
        for action in wrapped {
            let Action::MultisigPropose(proposal) = action else {
                panic!("fee action must be wrapped in a proposal");
            };
            assert_eq!(proposal.multisig, expected);
            assert_eq!(proposal.actions.len(), 1);
            assert!(proposal.actions[0].is_fee_admin_multisig_action());
            assert!(!proposal.actions[0].is_admin_multisig_action());
        }
    }

    #[test]
    fn wraps_user_admin_with_protocol_admin_multisig() {
        let target = Pubkey::new_unique();
        let wrapped = wrap_admin_actions(vec![Action::UserAdmin(UserAdmin {
            pubkey: target,
            maxorders: Some(500),
            global_maxorders: Some(10_000),
            meta: ActionMeta::default(),
        })]);

        let [Action::MultisigPropose(proposal)] = wrapped.as_slice() else {
            panic!("user admin action must be wrapped in an admin proposal");
        };
        assert_eq!(proposal.multisig, Pubkey::from_str(ADMIN_MULTISIG).unwrap());
        assert!(matches!(
            proposal.actions.as_slice(),
            [Action::UserAdmin(_)]
        ));
    }

    #[test]
    fn wraps_multisig_policy_updates_with_their_target_multisig() {
        let admin = Pubkey::from_str(ADMIN_MULTISIG).unwrap();
        let fee_admin = Pubkey::from_str(FEE_ADMIN_MULTISIG).unwrap();
        let ordinary = Pubkey::new_unique();

        for target in [admin, fee_admin, ordinary] {
            let wrapped = wrap_admin_actions(vec![Action::UpdateMultisigPolicy(
                bulk_client::msgs::UpdateMultisigPolicy {
                    multisig: target,
                    signers: Some(vec![Pubkey::new_unique()]),
                    threshold: None,
                    time_lock_secs: None,
                    proposal_lifetime_secs: None,
                    meta: ActionMeta::default(),
                },
            )]);

            let [Action::MultisigPropose(proposal)] = wrapped.as_slice() else {
                panic!("multisig policy update must be wrapped in a proposal");
            };
            assert_eq!(proposal.multisig, target);
            assert_eq!(proposal.proposal_lifetime_secs, None);
            assert!(matches!(
                proposal.actions.as_slice(),
                [Action::UpdateMultisigPolicy(update)] if update.multisig == target
            ));
        }
    }

    #[test]
    fn does_not_double_wrap_multisig_policy_update_proposal() {
        let target = Pubkey::new_unique();
        let proposal = Action::MultisigPropose(MultisigPropose {
            multisig: target,
            actions: vec![Action::UpdateMultisigPolicy(
                bulk_client::msgs::UpdateMultisigPolicy {
                    multisig: target,
                    signers: None,
                    threshold: Some(2),
                    time_lock_secs: None,
                    proposal_lifetime_secs: None,
                    meta: ActionMeta::default(),
                },
            )],
            proposal_lifetime_secs: None,
            meta: ActionMeta::default(),
        });

        assert!(matches!(
            wrap_admin_actions(vec![proposal]).as_slice(),
            [Action::MultisigPropose(_)]
        ));
    }

    #[test]
    fn formats_threshold_approval_and_failed_execution() {
        let common = serde_json::json!({
            "multisig": ADMIN_MULTISIG,
            "proposalId": 17,
            "approvals": 2,
            "rejections": 0,
            "threshold": 2,
            "signer": "Signer111"
        });
        let results = vec![
            response("proposalApproved", None, common.clone()),
            response("proposalReadyForExecution", None, common.clone()),
            response(
                "proposalFailed",
                Some("minimum tier exceeds active tier count"),
                common,
            ),
        ];

        let output = format_results(&results, &[]);

        assert!(output.contains("Approvals"));
        assert!(output.contains("Progress: 2 / 2 approvals"));
        assert!(output.contains("Approved by: Signer111"));
        assert!(output.contains("Outcome"));
        assert!(output.contains("Status: Proposal failed"));
        assert!(output.contains("Error: minimum tier exceeds active tier count"));
    }

    #[test]
    fn formats_approval_awaiting_threshold() {
        let result = response(
            "proposalApproved",
            None,
            serde_json::json!({
                "proposalId": 18,
                "approvals": 1,
                "rejections": 0,
                "threshold": 2
            }),
        );

        let output = format_results(&[result], &[]);

        assert!(output.contains("Progress: 1 / 2 approvals"));
        assert!(output.contains("Status: Proposal approved"));
    }

    #[test]
    fn formats_created_proposal_with_threshold_and_nested_action_debug() {
        let created = response(
            "proposalCreated",
            None,
            serde_json::json!({
                "proposalId": 23,
                "threshold": 2
            }),
        );
        let actions =
            vec!["PricingAdmin(PricingAdmin { instrument: \"BTC\", source: Bulk })".to_string()];

        let output = format_results(&[created], &actions);

        assert!(output.contains("Proposal Created"));
        assert!(output.contains("Proposal: 23"));
        assert!(output.contains("Required signers: 2"));
        assert!(output.contains("[0] PricingAdmin"));
        assert!(!output.contains("MultisigPropose"));
    }

    fn response(status: &str, message: Option<&str>, raw: serde_json::Value) -> Response {
        Response {
            order_id: None,
            status: status.to_owned(),
            message: message.map(str::to_owned),
            raw,
        }
    }
}
