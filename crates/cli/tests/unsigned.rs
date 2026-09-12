use bulk_client::transaction::{Action, SignatureDomain, Transaction, TransactionSigner};
use serde_json::Value;
use solana_pubkey::Pubkey;
use std::net::TcpListener;
use std::process::{Command, Output};

const ACCOUNT: &str = "11111111111111111111111111111111";

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bulk"))
        .env_remove("BULK_PRIVATE_KEY")
        .env_remove("BULK_SIGNATURE_DOMAIN")
        .args(args)
        .output()
        .expect("run bulk")
}

#[test]
fn unsigned_order_is_offline_and_matches_existing_signing() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/api/v1", listener.local_addr().unwrap());
    let signer =
        TransactionSigner::from_private_key(&bs58::encode([7u8; 32]).into_string()).unwrap();
    let signer_address = signer.public_key_b58();
    let out = run(&[
        "place",
        "Buy",
        "BTC-USD",
        "0.01@95000",
        "--unsigned",
        "--account",
        ACCOUNT,
        "--signer",
        &signer_address,
        "--nonce",
        "18446744073709551615",
        "--signature-domain",
        "testnet",
        "--api-url",
        &url,
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["nonce"], u64::MAX.to_string());
    assert_eq!(value["account"], ACCOUNT);
    assert_eq!(value["signer"], signer_address);
    assert_eq!(value["signatureDomain"], "testnet");
    assert_eq!(value["signingPayload"]["mode"], "raw");
    assert!(value.get("signature").is_none());
    let hex = value["signingPayload"]["data"].as_str().unwrap();
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let actions: Vec<Action> = serde_json::from_value(value["actions"].clone()).unwrap();
    assert_eq!(actions.len(), 1);
    let mut tx = Transaction {
        account: ACCOUNT.parse::<Pubkey>().unwrap(),
        signer: signer.public_key(),
        nonce: u64::MAX,
        actions,
        signature: Default::default(),
    };
    tx.sign(&signer, SignatureDomain::Testnet).unwrap();
    let external_signature = signer
        .sign_transaction_bytes(&bytes, SignatureDomain::Testnet)
        .unwrap();
    assert_eq!(tx.signature, external_signature);
    assert!(tx.verify(SignatureDomain::Testnet).unwrap());
    assert!(!tx.verify(SignatureDomain::Mainnet).unwrap());
    tx.nonce -= 1;
    assert!(!tx.verify(SignatureDomain::Testnet).unwrap());
}

#[test]
fn unsigned_commands_share_export_and_default_signer() {
    for command in [
        vec!["place", "Sell", "BTC-USD", "0.01", "--reduce-only"],
        vec!["cancel-all", "--instrument", "BTC-USD"],
        vec!["stop", "BTC-USD", "0.01", "90000"],
        vec!["update-leverage", "BTC-USD=2"],
        vec!["faucet"],
    ] {
        let mut args = command;
        args.extend([
            "--unsigned",
            "--account",
            ACCOUNT,
            "--signature-domain",
            "testnet",
        ]);
        let out = run(&args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["signer"], ACCOUNT);
        assert!(value["nonce"].as_str().unwrap().parse::<u64>().unwrap() > 0);
        assert!(!value["actions"].as_array().unwrap().is_empty());
    }
}

#[test]
fn unsigned_rejects_missing_context_and_unsupported_modes() {
    for args in [
        vec!["place", "Buy", "BTC-USD", "1", "--unsigned"],
        vec![
            "place",
            "Buy",
            "BTC-USD",
            "1",
            "--unsigned",
            "--account",
            ACCOUNT,
        ],
        vec!["place", "Buy", "BTC-USD", "1", "--account", ACCOUNT],
        vec![
            "place",
            "Buy",
            "BTC-USD",
            "1",
            "--unsigned",
            "--account",
            ACCOUNT,
            "--ledger",
        ],
        vec!["config", "show", "--unsigned", "--account", ACCOUNT],
        vec!["place", "Buy", "BTC-USD", "1"],
    ] {
        let out = run(&args);
        assert!(!out.status.success(), "unexpected success: {args:?}");
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn signed_progress_keeps_stdout_and_unsigned_keeps_json() {
    // Disposable public test seed; decline the preview before any HTTP submission.
    let out = Command::new(env!("CARGO_BIN_EXE_bulk"))
        .env("BULK_PRIVATE_KEY", "11111111111111111111111111111111")
        .env_remove("BULK_SIGNATURE_DOMAIN")
        .args([
            "place",
            "Buy",
            "BTC-USD",
            "0.01@95000",
            "--signature-domain",
            "testnet",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("Placing Limit Buy BTC-USD"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("transaction rejected by user"));
}

#[test]
fn unsigned_rejects_invalid_numbers_and_unstable_maps_without_output() {
    for command in [
        vec!["place", "Buy", "BTC-USD", "NaN@95000"],
        vec!["place", "Buy", "BTC-USD", "inf@95000"],
        vec!["place", "Buy", "BTC-USD", "0.000000001@95000"],
        vec!["place", "Buy", "BTC-USD", "1e300@95000"],
        vec!["place", "Buy", "BTC-USD", "1", "--slippage", "NaN"],
        vec!["stop", "BTC-USD", "1", "95000", "--limit", "NaN"],
        vec!["update-leverage", "BTC-USD=NaN"],
        vec!["update-leverage", "BTC-USD=2", "ETH-USD=3"],
        vec!["faucet", "NaN"],
        vec!["user-admin", ACCOUNT, "--maxorders", "500"],
    ] {
        let mut args = command;
        args.extend([
            "--unsigned",
            "--account",
            ACCOUNT,
            "--signature-domain",
            "testnet",
        ]);
        let out = run(&args);
        assert!(!out.status.success(), "unexpected export: {args:?}");
        assert!(out.stdout.is_empty(), "partial JSON: {args:?}");
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(
            error.contains("unsigned"),
            "unexpected error for {args:?}: {error}"
        );
        assert!(!error.contains("panicked"));
    }
}

#[test]
fn supported_exports_reconstruct_identical_signing_bytes() {
    for command in [
        vec!["place", "Buy", "BTC-USD", "0.01@95000"],
        vec!["place", "Sell", "BTC-USD", "0.01", "--reduce-only"],
        vec!["stop", "BTC-USD", "1", "95000", "--limit", "94000"],
        vec!["update-leverage", "BTC-USD=2"],
        vec!["update-multisig", ACCOUNT, "--threshold", "2"],
        vec!["multisig-approve", ACCOUNT, "123"],
        vec!["faucet", "10"],
    ] {
        let mut args = command;
        args.extend([
            "--unsigned",
            "--account",
            ACCOUNT,
            "--nonce",
            "123",
            "--signature-domain",
            "testnet",
        ]);
        let mut previous = None;
        for _ in 0..3 {
            let out = run(&args);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let value: Value = serde_json::from_slice(&out.stdout).unwrap();
            let actions: Vec<Action> = serde_json::from_value(value["actions"].clone()).unwrap();
            let bytes = Transaction::raw_signable_bytes(
                SignatureDomain::Testnet,
                ACCOUNT.parse().unwrap(),
                123,
                &actions,
            )
            .unwrap();
            let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(value["signingPayload"]["data"], hex);
            if let Some(previous) = previous {
                assert_eq!(value, previous);
            }
            previous = Some(value);
        }
    }
}

#[cfg(unix)]
#[test]
fn unsigned_closed_stdout_returns_an_error_without_panicking() {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    let (reader, writer) = UnixStream::pair().unwrap();
    drop(reader);
    let fd: OwnedFd = writer.into();
    let out = Command::new(env!("CARGO_BIN_EXE_bulk"))
        .env_remove("BULK_PRIVATE_KEY")
        .env_remove("BULK_SIGNATURE_DOMAIN")
        .args([
            "place",
            "Buy",
            "BTC-USD",
            "1",
            "--unsigned",
            "--account",
            ACCOUNT,
            "--signature-domain",
            "testnet",
        ])
        .stdout(std::process::Stdio::from(fd))
        .output()
        .unwrap();
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(!error.contains("panicked"), "{error}");
    assert!(error.contains("Broken pipe"), "{error}");
}
