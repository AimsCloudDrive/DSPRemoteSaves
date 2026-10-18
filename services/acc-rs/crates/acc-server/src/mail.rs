//! 邮件发送：有界队列 + 固定 worker + SMTP 熔断 + 日配额。
//!
//! `mail.enabled = false` 时不连 SMTP，验证码直接写日志（仅开发环境使用）。

use crate::cache::Cache;
use crate::config::MailConfig;
use crate::error::{AppError, ErrorCode};
use crate::i18n::Lang;
use lettre::message::header::ContentType;
use lettre::message::{Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};
use tracing::{error, info, warn};

#[derive(Debug, Clone)]
pub enum MailContent {
    Code { code: String, purpose: String },
    Notice { subject: String, text: String },
}

#[derive(Debug, Clone)]
pub struct MailJob {
    pub to: String,
    pub lang: Lang,
    pub content: MailContent,
}

struct Inner {
    cfg: MailConfig,
    cache: Cache,
    transport: Option<AsyncSmtpTransport<Tokio1Executor>>,
    consecutive_failures: AtomicU32,
    open_until: Mutex<Option<Instant>>,
}

#[derive(Clone)]
pub struct Mailer {
    tx: mpsc::Sender<MailJob>,
    inner: Arc<Inner>,
}

impl Mailer {
    pub fn build(cfg: MailConfig, cache: Cache, capacity: usize) -> (Self, mpsc::Receiver<MailJob>) {
        let transport = if cfg.enabled {
            match build_transport(&cfg) {
                Ok(t) => Some(t),
                Err(err) => {
                    error!(error = %err, "SMTP 传输初始化失败，邮件功能不可用");
                    None
                }
            }
        } else {
            None
        };
        info!(
            enabled = cfg.enabled,
            host = %cfg.smtp_host,
            port = cfg.smtp_port,
            "邮件通道初始化"
        );
        let (tx, rx) = mpsc::channel(capacity.max(1));
        let mailer = Self {
            tx,
            inner: Arc::new(Inner {
                cfg,
                cache,
                transport,
                consecutive_failures: AtomicU32::new(0),
                open_until: Mutex::new(None),
            }),
        };
        (mailer, rx)
    }

    /// 启动 worker；返回 JoinHandle 便于优雅退出。
    pub fn start(&self, mut rx: mpsc::Receiver<MailJob>) -> tokio::task::JoinHandle<()> {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            while let Some(job) = rx.recv().await {
                if let Err(err) = deliver(&inner, &job).await {
                    error!(to = %mask(&job.to), error = %err, "邮件发送失败");
                }
            }
            info!("邮件 worker 退出");
        })
    }

    /// 入队；队列满或熔断时返回错误，调用方转成 429/503。
    pub async fn enqueue(&self, job: MailJob) -> Result<(), AppError> {
        if !self.inner.cfg.enabled {
            match &job.content {
                MailContent::Code { code, purpose } => info!(
                    to = %mask(&job.to),
                    purpose = %purpose,
                    code = %code,
                    "邮件通道未启用，验证码仅打印到日志"
                ),
                MailContent::Notice { subject, .. } => info!(
                    to = %mask(&job.to),
                    subject = %subject,
                    "邮件通道未启用，通知仅打印到日志"
                ),
            }
            return Ok(());
        }
        if self.is_open().await {
            return Err(AppError::new(ErrorCode::MailUnavailable));
        }
        self.tx.try_send(job).map_err(|err| match err {
            mpsc::error::TrySendError::Full(_) => {
                AppError::new(ErrorCode::RateLimited).with_retry(30)
            }
            mpsc::error::TrySendError::Closed(_) => AppError::new(ErrorCode::MailUnavailable),
        })
    }

    async fn is_open(&self) -> bool {
        let guard = self.inner.open_until.lock().await;
        match *guard {
            Some(until) if until > Instant::now() => true,
            Some(_) => false,
            None => false,
        }
    }

    /// 当日剩余额度（用于 80% 告警与 100% 拒绝）。
    pub async fn remaining_quota(&self) -> u32 {
        let used = self.today_count().await;
        self.inner.cfg.daily_limit.saturating_sub(used)
    }

    async fn today_count(&self) -> u32 {
        let key = daily_key();
        match self.inner.cache.get_raw(&key).await {
            Some(v) => v.parse().unwrap_or(0),
            None => 0,
        }
    }
}

fn daily_key() -> String {
    use chrono::Local;
    format!("mail:sent:{}", Local::now().format("%Y%m%d"))
}

fn build_transport(
    cfg: &MailConfig,
) -> Result<AsyncSmtpTransport<Tokio1Executor>, Box<dyn std::error::Error>> {
    let tls_params = TlsParameters::new(cfg.smtp_host.clone())?;
    let tls = if cfg.smtp_tls.eq_ignore_ascii_case("starttls") {
        Tls::Required(tls_params)
    } else {
        Tls::Wrapper(tls_params)
    };

    let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.smtp_host)
        .port(cfg.smtp_port)
        .tls(tls)
        .timeout(Some(Duration::from_secs(15)));

    if !cfg.username.is_empty() {
        builder = builder.credentials(Credentials::new(
            cfg.username.clone(),
            cfg.password.clone(),
        ));
    }
    Ok(builder.build())
}

async fn deliver(inner: &Inner, job: &MailJob) -> Result<(), String> {
    // 日配额：企业邮免费版约 500 封/天，超过后直接失败，避免 SMTP 被封
    let used = inner
        .cache
        .incr_with_ttl(&daily_key(), 2 * 24 * 3600)
        .await as u32;
    if used > inner.cfg.daily_limit {
        return Err("mail_quota_exhausted".into());
    }
    if used * 100 >= inner.cfg.daily_limit * 80 {
        warn!(used, limit = inner.cfg.daily_limit, "邮件日配额已使用 80%");
    }

    let Some(transport) = inner.transport.as_ref() else {
        return Err("SMTP 未初始化".into());
    };

    let message = build_message(&inner.cfg, job)?;
    match transport.send(message).await {
        Ok(_) => {
            inner.consecutive_failures.store(0, Ordering::Relaxed);
            Ok(())
        }
        Err(err) => {
            let failures = inner.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
            if failures >= 5 {
                let mut guard = inner.open_until.lock().await;
                *guard = Some(Instant::now() + Duration::from_secs(60));
                warn!("SMTP 连续失败 {failures} 次，熔断 60 秒");
            }
            Err(err.to_string())
        }
    }
}

fn build_message(cfg: &MailConfig, job: &MailJob) -> Result<Message, String> {
    let from_addr = cfg
        .from_address
        .parse()
        .map_err(|_| "from_address 非法".to_string())?;
    let from = Mailbox::new(Some(cfg.from_name.clone()), from_addr);
    let to = job.to.parse().map_err(|_| "收件地址非法".to_string())?;

    let (subject, text, html) = render(job);
    Message::builder()
        .from(from)
        .to(Mailbox::new(None, to))
        .subject(subject)
        .multipart(
            MultiPart::alternative()
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_PLAIN)
                        .body(text),
                )
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_HTML)
                        .body(html),
                ),
        )
        .map_err(|e| e.to_string())
}

fn render(job: &MailJob) -> (String, String, String) {
    let (code, purpose) = match &job.content {
        MailContent::Code { code, purpose } => (code.as_str(), purpose.as_str()),
        MailContent::Notice { subject, text } => {
            let html = format!(
                r#"<div style="font-family:-apple-system,Segoe UI,Roboto,Helvetica,Arial,sans-serif;max-width:520px;margin:0 auto;padding:24px;color:#1f2328"><p>{}</p></div>"#,
                escape_html(text)
            );
            return (subject.clone(), text.clone(), html);
        }
    };

    let action = match (purpose, job.lang) {
        ("register", Lang::Zh) => "注册账号",
        ("register", Lang::En) => "create your account",
        ("reset", Lang::Zh) => "重置密码",
        ("reset", Lang::En) => "reset your password",
        ("change_email", Lang::Zh) => "更换邮箱",
        ("change_email", Lang::En) => "change your email",
        (_, Lang::Zh) => "安全验证",
        (_, Lang::En) => "security verification",
    };

    match job.lang {
        Lang::Zh => {
            let subject = format!("DSP 云存档验证码：{code}");
            let text = format!(
                "你正在{action}。\n\n验证码：{code}\n\n验证码 5 分钟内有效，仅可使用一次。\n如非本人操作，请忽略本邮件并尽快修改密码。\n\n—— DSP 云存档"
            );
            let html = format!(
                r#"<div style="font-family:-apple-system,Segoe UI,Roboto,Helvetica,Arial,sans-serif;max-width:520px;margin:0 auto;padding:24px;color:#1f2328">
  <h2 style="margin:0 0 16px">DSP 云存档</h2>
  <p>你正在<b>{action}</b>，本次验证码：</p>
  <p style="font-size:32px;letter-spacing:8px;font-weight:700;margin:16px 0">{code}</p>
  <p style="color:#57606a">验证码 5 分钟内有效，仅可使用一次。</p>
  <p style="color:#57606a">如非本人操作，请忽略本邮件并尽快修改密码。</p>
  <hr style="border:none;border-top:1px solid #d0d7de;margin:24px 0">
  <p style="color:#8c959f;font-size:12px">此邮件由系统自动发送，请勿直接回复。</p>
</div>"#,
                action = action,
                code = code
            );
            (subject, text, html)
        }
        Lang::En => {
            let subject = format!("DSP Cloud Saves verification code: {code}");
            let text = format!(
                "You are trying to {action}.\n\nVerification code: {code}\n\nThe code expires in 5 minutes and can be used once.\nIf this was not you, ignore this email and change your password.\n\n— DSP Cloud Saves"
            );
            let html = format!(
                r#"<div style="font-family:-apple-system,Segoe UI,Roboto,Helvetica,Arial,sans-serif;max-width:520px;margin:0 auto;padding:24px;color:#1f2328">
  <h2 style="margin:0 0 16px">DSP Cloud Saves</h2>
  <p>You are trying to <b>{action}</b>. Your verification code is:</p>
  <p style="font-size:32px;letter-spacing:8px;font-weight:700;margin:16px 0">{code}</p>
  <p style="color:#57606a">The code expires in 5 minutes and can be used once.</p>
  <p style="color:#57606a">If this was not you, ignore this email and change your password.</p>
  <hr style="border:none;border-top:1px solid #d0d7de;margin:24px 0">
  <p style="color:#8c959f;font-size:12px">This is an automated message, please do not reply.</p>
</div>"#,
                action = action,
                code = code
            );
            (subject, text, html)
        }
    }
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn mask(email: &str) -> String {
    crate::util::mask_email(email)
}
