//! Dedicated server-held Agent keys; only ApproveAgent is signed in the browser.
use crate::web::AppState;
use alloy::{
    primitives::{Address, Signature, B256, U256},
    signers::local::PrivateKeySigner,
};
use alloy_sol_types::{sol, Eip712Domain, SolStruct};
use anyhow::{Context, Result};
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
};

const TTL_MS: u64 = 10 * 60 * 1000;
const NAME: &str = "StarsStrategy";
sol! {
    struct ApproveAgent {
        string txflowNetwork;
        uint32 chainId;
        uint32 apiVersion;
        address agentAddress;
        string agentName;
        uint64 nonce;
    }
}

#[derive(Serialize, Deserialize)]
struct Pending {
    id: String,
    account: Address,
    agent: Address,
    signature_chain_id: u64,
    nonce: u64,
    phase: String,
}

#[derive(Deserialize)]
pub struct PrepareBody {
    account: String,
    signature_chain_id: u64,
}
#[derive(Deserialize)]
pub struct ApproveBody {
    id: String,
    signature: String,
}

pub async fn script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../static/txflow-agent.js"),
    )
}

// These wallet endpoints deliberately reject cross-origin and non-browser requests.
fn same_origin(headers: &HeaderMap) -> Result<()> {
    let origin = headers
        .get(header::ORIGIN)
        .context("缺少浏览器 Origin")?
        .to_str()?;
    let origin = reqwest::Url::parse(origin)?;
    anyhow::ensure!(matches!(origin.scheme(), "http" | "https"), "无效 Origin");
    let host = headers.get(header::HOST).context("缺少 Host")?.to_str()?;
    let expected = reqwest::Url::parse(&format!("{}://{host}", origin.scheme()))?;
    anyhow::ensure!(origin.origin() == expected.origin(), "只允许本站发起授权");
    if let Some(site) = headers.get("sec-fetch-site") {
        anyhow::ensure!(site == "same-origin", "只允许本站发起授权");
    }
    Ok(())
}
fn directory(state: &AppState) -> PathBuf {
    state
        .live_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("txflow-agents")
}
fn record_path(dir: &Path, id: &str) -> Result<PathBuf> {
    let id = uuid::Uuid::parse_str(id).context("无效的授权编号")?;
    Ok(dir.join(format!("{id}.json")))
}
fn key_path(dir: &Path, p: &Pending) -> PathBuf {
    dir.join(format!("{}.key", p.id))
}

fn private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).context("无法创建 Agent 密钥目录")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn private_file(path: &Path, contents: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}
// Atomic phase updates prevent a restart from re-sending a previously submitted signature.
fn save_pending(dir: &Path, p: &Pending) -> Result<()> {
    let dest = record_path(dir, &p.id)?;
    let temp = dir.join(format!("{}.tmp", uuid::Uuid::new_v4()));
    private_file(&temp, &serde_json::to_vec(p)?)?;
    fs::rename(&temp, &dest)?;
    fs::File::open(dir)?.sync_all()?;
    Ok(())
}
fn create_pending(dir: &Path, account: Address, chain: u64) -> Result<Pending> {
    anyhow::ensure!(
        account != Address::ZERO && chain > 0,
        "账户地址或钱包链 ID 无效"
    );
    private_dir(dir)?;
    let signer = PrivateKeySigner::random();
    let p = Pending {
        id: uuid::Uuid::new_v4().to_string(),
        account,
        agent: signer.address(),
        signature_chain_id: chain,
        nonce: crate::live::now_ms_pub() as u64,
        phase: "prepared".into(),
    };
    private_file(
        &key_path(dir, &p),
        format!("{:#x}\n", signer.to_bytes()).as_bytes(),
    )?;
    save_pending(dir, &p)?;
    Ok(p)
}
impl Pending {
    fn typed_data(&self) -> Value {
        json!({
            "types": {
                "EIP712Domain": [
                    {"name":"name","type":"string"}, {"name":"version","type":"string"},
                    {"name":"chainId","type":"uint256"}, {"name":"verifyingContract","type":"address"}
                ],
                "ApproveAgent": [
                    {"name":"txflowNetwork","type":"string"}, {"name":"chainId","type":"uint32"},
                    {"name":"apiVersion","type":"uint32"}, {"name":"agentAddress","type":"address"},
                    {"name":"agentName","type":"string"}, {"name":"nonce","type":"uint64"}
                ]
            },
            "domain": {"name":"TxFlow-Mainnet","version":"1","chainId":self.signature_chain_id,
                "verifyingContract":Address::ZERO},
            "primaryType":"ApproveAgent",
            "message": {"txflowNetwork":"TxFlow-Mainnet","chainId":869,"apiVersion":1,
                "agentAddress":self.agent,"agentName":NAME,"nonce":self.nonce}
        })
    }
    fn digest(&self) -> B256 {
        ApproveAgent {
            txflowNetwork: "TxFlow-Mainnet".into(),
            chainId: 869,
            apiVersion: 1,
            agentAddress: self.agent,
            agentName: NAME.into(),
            nonce: self.nonce,
        }
        .eip712_signing_hash(&Eip712Domain {
            name: Some("TxFlow-Mainnet".into()),
            version: Some("1".into()),
            chain_id: Some(U256::from(self.signature_chain_id)),
            verifying_contract: Some(Address::ZERO),
            salt: None,
        })
    }
    fn validate_signature(&self, signature: &str) -> Result<Signature> {
        let sig = Signature::from_str(signature).context("钱包签名格式错误")?;
        anyhow::ensure!(
            sig.recover_address_from_prehash(&self.digest())? == self.account,
            "签名与主账户不匹配，请使用准备授权时连接的钱包"
        );
        Ok(sig)
    }
    fn exchange_body(&self, sig: Signature) -> Value {
        let mut action = self.typed_data()["message"].clone();
        action["type"] = json!("approveAgent");
        json!({"action":action, "nonce":self.nonce,"signatureChainId":self.signature_chain_id,
            "signature":{"r":format!("{:#066x}",sig.r()),"s":format!("{:#066x}",sig.s()),
                "v":27 + u8::from(sig.v())}})
    }
}
fn failure(status: StatusCode, error: impl std::fmt::Display) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"ok":false,"error":error.to_string()})),
    )
        .into_response()
}
fn success(body: Value) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}
pub async fn prepare(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PrepareBody>,
) -> Response {
    if let Err(e) = same_origin(&headers) {
        return failure(StatusCode::FORBIDDEN, e);
    }
    let _guard = match state.exec_gate.try_lock() {
        Ok(g) => g,
        Err(_) => return failure(StatusCode::CONFLICT, "策略正在执行，请稍后授权"),
    };
    let config = state.live.lock().await.config.clone();
    if config.armed || config.auto_run {
        return failure(
            StatusCode::CONFLICT,
            "请先关闭实盘和自动调仓并保存，再更换 Agent",
        );
    }
    let account = match Address::from_str(body.account.trim()) {
        Ok(a) if a != Address::ZERO => a,
        _ => return failure(StatusCode::BAD_REQUEST, "无效的主账户地址"),
    };
    match create_pending(&directory(&state), account, body.signature_chain_id) {
        Ok(p) => success(json!({"ok":true,"id":p.id,"agent_address":p.agent,
            "account":p.account,"key_path":key_path(&directory(&state), &p),
            "typed_data":p.typed_data(),"expires_at":p.nonce+TTL_MS})),
        Err(e) => failure(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub async fn approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ApproveBody>,
) -> Response {
    if let Err(e) = same_origin(&headers) {
        return failure(StatusCode::FORBIDDEN, e);
    }
    // Continue after a browser disconnect, so the authorization and local save stay together.
    let result = tokio::spawn(async move {
        finish_approval(state, body, "https://api.txflow.com/exchange").await
    })
    .await;
    match result {
        Ok(Ok(v)) => success(v),
        Ok(Err(e)) => failure(StatusCode::BAD_REQUEST, e),
        Err(_) => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "授权任务中断，请核对 TxFlow 账户后重新授权",
        ),
    }
}
async fn finish_approval(state: AppState, body: ApproveBody, endpoint: &str) -> Result<Value> {
    let _guard = state
        .exec_gate
        .try_lock()
        .context("策略或授权正在执行，请稍后操作")?;
    let config = state.live.lock().await.config.clone();
    anyhow::ensure!(
        !config.armed && !config.auto_run,
        "请先关闭实盘和自动调仓并保存"
    );
    let dir = directory(&state);
    let path = record_path(&dir, &body.id)?;
    let mut p: Pending =
        serde_json::from_slice(&fs::read(path).context("未找到授权记录，请重新生成")?)?;
    anyhow::ensure!(p.id == body.id, "授权编号不匹配");
    let sig = p.validate_signature(&body.signature)?;
    let key = key_path(&dir, &p);
    let signer = PrivateKeySigner::from_str(fs::read_to_string(&key)?.trim())
        .map_err(|_| anyhow::anyhow!("服务器 Agent 密钥文件损坏"))?;
    anyhow::ensure!(
        signer.address() == p.agent,
        "服务器 Agent 密钥与授权地址不匹配"
    );
    if p.phase != "authorized" {
        anyhow::ensure!(
            p.phase == "prepared",
            "这次授权已提交，结果可能未知；不会重发，请核对后重新生成授权"
        );
        let now = crate::live::now_ms_pub() as u64;
        anyhow::ensure!(
            now >= p.nonce && now - p.nonce <= TTL_MS,
            "授权已过期，请重新生成并签名"
        );
        p.phase = "submitted".into();
        save_pending(&dir, &p)?;
        crate::txflow::approve_agent(&state.http, &p.exchange_body(sig), endpoint)
            .await
            .context("授权失败或结果未知，未启用策略；请核对账户后重新授权")?;
        p.phase = "authorized".into();
        save_pending(&dir, &p).context("TxFlow 已接受授权，但本地回执保存失败，请勿启用实盘")?;
    }
    let mut st = state.live.lock().await;
    let previous = st.clone();
    st.config.account = p.account.to_string();
    st.config.key_path = key.to_string_lossy().into_owned();
    st.config.armed = false;
    st.config.auto_run = false;
    if let Err(e) = st.save(&state.live_path) {
        *st = previous;
        anyhow::bail!(
            "TxFlow 已授权，保存配置失败: {e}。修复目录权限后点击重试保存即可，不会重复发授权"
        );
    }
    Ok(json!({"ok":true,"agent_address":p.agent,"config":st.config}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::SignerSync;
    #[test]
    fn server_key_is_private_and_never_in_public_challenge() {
        let dir = std::env::temp_dir().join(format!("stars-agent-{}", uuid::Uuid::new_v4()));
        let p = create_pending(&dir, Address::from([1u8; 20]), 42161).unwrap();
        let key = fs::read_to_string(key_path(&dir, &p)).unwrap();
        let signer = PrivateKeySigner::from_str(key.trim()).unwrap();
        assert_eq!(signer.address(), p.agent);
        assert!(!p.typed_data().to_string().contains(key.trim()));
        assert!(!fs::read_to_string(record_path(&dir, &p.id).unwrap())
            .unwrap()
            .contains(key.trim()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(key_path(&dir, &p))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert!(record_path(&dir, "../../secret").is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn signature_binds_account_agent_nonce_and_wallet_chain() {
        let wallet = PrivateKeySigner::random();
        let mut p = Pending {
            id: uuid::Uuid::new_v4().to_string(),
            account: wallet.address(),
            agent: Address::from([2u8; 20]),
            signature_chain_id: 42161,
            nonce: 123456789,
            phase: "prepared".into(),
        };
        let sig = wallet.sign_hash_sync(&p.digest()).unwrap().to_string();
        // Independent ethers TypedDataEncoder fixture following the official wallet hook.
        assert_eq!(
            format!("{:#x}", p.digest()),
            "0xb99ceee064bb770314b8016fd16e0d170734ad03a21faed3bfe877acc229f4d6"
        );
        let parsed = p.validate_signature(&sig).unwrap();
        assert_eq!(p.exchange_body(parsed)["action"]["type"], "approveAgent");
        p.nonce += 1;
        assert!(p.validate_signature(&sig).is_err());
        p.nonce -= 1;
        p.signature_chain_id = 1;
        assert!(p.validate_signature(&sig).is_err());
        p.signature_chain_id = 42161;
        p.agent = Address::from([3u8; 20]);
        assert!(p.validate_signature(&sig).is_err());
    }
    #[test]
    fn authorization_rejects_cross_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "stars.example".parse().unwrap());
        assert!(same_origin(&headers).is_err());
        headers.insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        assert!(same_origin(&headers).is_err());
        headers.insert(header::ORIGIN, "https://stars.example".parse().unwrap());
        assert!(same_origin(&headers).is_ok());
        headers.insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert!(same_origin(&headers).is_err());
    }

    fn test_state(root: &Path) -> AppState {
        use std::sync::Arc;
        use tokio::sync::Mutex;
        let mut live = crate::live::LiveState::default();
        live.config.txflow = true;
        AppState {
            store: Arc::new(crate::store::Store::open(&root.join("tx.sqlite")).unwrap()),
            paper: Arc::new(Mutex::new(crate::paper::PaperState::default())),
            paper_path: Arc::new(root.join("paper.json")),
            live: Arc::new(Mutex::new(live)),
            live_path: Arc::new(root.join("txflow-live.json")),
            meta: Arc::new(Mutex::new(crate::web::MetaCache::default())),
            refresh: Arc::new(Mutex::new(crate::web::RefreshStatus::default())),
            http: reqwest::Client::new(),
            exec_gate: Arc::new(Mutex::new(())),
        }
    }
    #[tokio::test]
    async fn approval_receipt_persists_configuration_and_does_not_resubmit() {
        use axum::{routing::post, Router};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let root = std::env::temp_dir().join(format!("stars-agent-flow-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let state = test_state(&root);
        let wallet = PrivateKeySigner::random();
        let p = create_pending(&directory(&state), wallet.address(), 42161).unwrap();
        let signature = wallet.sign_hash_sync(&p.digest()).unwrap().to_string();
        let expected = p.exchange_body(p.validate_signature(&signature).unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_http = calls.clone();
        let app = Router::new().route(
            "/exchange",
            post(move |Json(body): Json<Value>| {
                let calls = calls_http.clone();
                let expected = expected.clone();
                async move {
                    assert_eq!(body, expected);
                    calls.fetch_add(1, Ordering::SeqCst);
                    Json(json!({"code":200,"data":{"status":"ok"}}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/exchange", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        // Simulate a permissions/path problem after a successful exchange receipt.
        let bad_state = AppState {
            live_path: Arc::new(root.join("txflow-agents")),
            ..state.clone()
        };
        let dir = directory(&state);
        // Both live paths have the same parent and therefore the same managed Agent directory.
        let error = finish_approval(
            bad_state,
            ApproveBody {
                id: p.id.clone(),
                signature: signature.clone(),
            },
            &endpoint,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("保存配置失败"));
        assert!(state.live.lock().await.config.account.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let record: Pending =
            serde_json::from_slice(&fs::read(record_path(&dir, &p.id).unwrap()).unwrap()).unwrap();
        assert_eq!(record.phase, "authorized");
        finish_approval(
            state.clone(),
            ApproveBody {
                id: p.id.clone(),
                signature: signature.clone(),
            },
            &endpoint,
        )
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let saved = crate::live::LiveState::load(&state.live_path);
        assert_eq!(saved.config.account, wallet.address().to_string());
        assert_eq!(saved.config.key_path, key_path(&dir, &p).to_string_lossy());
        assert!(!saved.config.armed && !saved.config.auto_run);
        // A signed challenge cannot be applied while live trading is enabled.
        state.live.lock().await.config.armed = true;
        assert!(finish_approval(
            state.clone(),
            ApproveBody {
                id: p.id,
                signature
            },
            &endpoint
        )
        .await
        .is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
        drop(state);
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn unknown_or_expired_approval_cannot_be_replayed() {
        let root =
            std::env::temp_dir().join(format!("stars-agent-replay-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let state = test_state(&root);
        let dir = directory(&state);
        let wallet = PrivateKeySigner::random();
        let mut p = create_pending(&dir, wallet.address(), 1).unwrap();
        // A transport failure must leave a durable submitted marker.
        let signature = wallet.sign_hash_sync(&p.digest()).unwrap().to_string();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unavailable = format!("http://{}/exchange", listener.local_addr().unwrap());
        drop(listener);
        assert!(finish_approval(
            state.clone(),
            ApproveBody {
                id: p.id.clone(),
                signature: signature.clone()
            },
            &unavailable
        )
        .await
        .is_err());
        let record: Pending =
            serde_json::from_slice(&fs::read(record_path(&dir, &p.id).unwrap()).unwrap()).unwrap();
        assert_eq!(record.phase, "submitted");
        let error = finish_approval(
            state.clone(),
            ApproveBody {
                id: p.id.clone(),
                signature,
            },
            &unavailable,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("不会重发"));
        p.phase = "prepared".into();
        p.nonce -= TTL_MS + 1;
        save_pending(&dir, &p).unwrap();
        let signature = wallet.sign_hash_sync(&p.digest()).unwrap().to_string();
        let error = finish_approval(
            state.clone(),
            ApproveBody {
                id: p.id.clone(),
                signature,
            },
            &unavailable,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("过期"));
        assert!(state.live.lock().await.config.account.is_empty());
        drop(state);
        fs::remove_dir_all(root).unwrap();
    }
}
