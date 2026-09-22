//! An HTTP endpoint that prints what people send it.
//!
//! One printer, one roll of paper, one USB claim — so every guard here exists
//! because the thing on the other end is physical and finite:
//!
//! - a shared password, checked in constant time
//! - a cooldown and a daily allowance per client, and a cap for the whole day
//! - a length limit, and control characters stripped before anything is sent
//! - a pause file that stops printing without stopping the service
//! - one print at a time, because a second USB claim would simply fail
//!
//! It serves its own page, so the browser talks to one origin and there is no
//! CORS to get wrong. Run it behind a Cloudflare tunnel; see docs/network.md.

use axum::{
    Json, Router,
    body::Bytes,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
};
use chrono::{Datelike, Local};
use clap::Parser;
use receipt::linear;
use receipt::printer::{Align, Font, Printer, Style, WIDTH, WIDTH_BIG};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The page, compiled in so the binary is the whole deployment.
const PAGE: &str = include_str!("../../web/index.html");

/// The horizontal rule on a printed slip: a solid cp437 line.
const RULE: char = '\u{2550}';

#[derive(Parser)]
#[command(version, about = "HTTP endpoint for the receipt printer")]
struct Args {
    /// Address to listen on
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: SocketAddr,

    /// Shared password callers must send. Prefer RECEIPT_PASSWORD in the
    /// environment — an argument is visible to anyone who can run `ps`.
    #[arg(long, env = "RECEIPT_PASSWORD", hide_env_values = true)]
    password: String,

    /// Longest message accepted, in characters
    #[arg(long, default_value_t = 600)]
    max_chars: usize,

    /// Seconds a client must wait between prints; 0 for no wait
    #[arg(long, default_value_t = 0)]
    cooldown: u64,

    /// Prints allowed per client per day; 0 for no limit
    #[arg(long, default_value_t = 0)]
    per_client_daily: usize,

    /// Prints allowed across everyone per day; 0 for no limit. Not a rationing
    /// of the paper so much as a stop on a runaway loop.
    #[arg(long, default_value_t = 2000)]
    daily_cap: usize,

    /// While this file exists nothing prints; the page says so
    #[arg(long, default_value = "/var/lib/receipt-server/paused")]
    pause_file: PathBuf,

    /// Signing secret from the Linear webhook. Without it the webhook route
    /// refuses everything, because an unverified caller is a stranger.
    #[arg(long, env = "LINEAR_WEBHOOK_SECRET", hide_env_values = true)]
    linear_secret: Option<String>,

    /// Linear personal API key. A comment webhook carries only an issue id, so
    /// the issue itself has to be fetched.
    #[arg(long, env = "LINEAR_API_KEY", hide_env_values = true)]
    linear_key: Option<String>,

    /// Attempts to print a Linear slip if the printer is busy or unhappy,
    /// 30s apart. Covers a paper change; does not survive a restart.
    #[arg(long, default_value_t = 6)]
    linear_retries: u32,
}

/// What the whole service knows. The gate is a plain mutex because every
/// operation on it is a few microseconds of bookkeeping; the printer is an
/// async mutex because holding it spans a blocking USB write.
struct App {
    args: Args,
    gate: Mutex<Gate>,
    printer: tokio::sync::Mutex<()>,
    /// None when the two Linear secrets were not supplied; the route then
    /// says so rather than pretending to work.
    linear: Option<Linear>,
    /// Deliveries already acted on. Linear retries, and a retry must not mean
    /// a second receipt.
    seen: Mutex<VecDeque<String>>,
}

struct Linear {
    client: linear::Client,
    secret: String,
    /// Whoever the API key belongs to. Only this person's comments print.
    viewer_id: String,
}

/// Rate limiting, per client and in total. `day` is a local-time ordinal, so
/// allowances reset at midnight where the printer is rather than at UTC.
struct Gate {
    day: i32,
    total_today: usize,
    clients: HashMap<IpAddr, Client>,
}

struct Client {
    last: Instant,
    today: usize,
}

/// Why a print was refused. Each maps to the status code a caller should see.
enum Refusal {
    Password,
    Empty,
    TooLong(usize),
    Cooldown(u64),
    ClientDaily,
    DailyCap,
    Paused,
    Printer(String),
}

impl Refusal {
    fn parts(&self) -> (StatusCode, String) {
        match self {
            Refusal::Password => (StatusCode::UNAUTHORIZED, "wrong password".into()),
            Refusal::Empty => (StatusCode::BAD_REQUEST, "nothing to print".into()),
            Refusal::TooLong(max) => (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("too long — {max} characters at most"),
            ),
            Refusal::Cooldown(secs) => (
                StatusCode::TOO_MANY_REQUESTS,
                format!("hold on — {secs}s before the next one"),
            ),
            Refusal::ClientDaily => (
                StatusCode::TOO_MANY_REQUESTS,
                "that is your lot for today".into(),
            ),
            Refusal::DailyCap => (
                StatusCode::TOO_MANY_REQUESTS,
                "the printer has had enough for one day".into(),
            ),
            Refusal::Paused => (StatusCode::SERVICE_UNAVAILABLE, "printing is paused".into()),
            Refusal::Printer(why) => (StatusCode::SERVICE_UNAVAILABLE, why.clone()),
        }
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> axum::response::Response {
        let (code, error) = self.parts();
        (code, Json(Reply { ok: false, message: error })).into_response()
    }
}

#[derive(Deserialize)]
struct Submission {
    password: String,
    #[serde(default)]
    name: String,
    text: String,
}

#[derive(Serialize)]
struct Reply {
    ok: bool,
    message: String,
}

#[derive(Serialize)]
struct Status {
    ready: bool,
    detail: String,
    paused: bool,
    remaining_today: Option<usize>,
    max_chars: usize,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let listen = args.listen;

    if args.password.trim().is_empty() {
        eprintln!("receipt-server: the password must not be empty");
        std::process::exit(1);
    }

    // Fail at startup rather than on the first submission: a service that
    // answers but cannot print is worse than one that never came up.
    match Printer::open() {
        Ok(p) => {
            let problems = p.status();
            if problems.is_empty() {
                println!("printer ready");
            } else {
                println!("printer reachable but: {}", problems.join("; "));
            }
        }
        Err(e) => {
            eprintln!("receipt-server: cannot open the printer: {e}");
            std::process::exit(1);
        }
    }

    // Resolve "me" from the API key rather than asking for a user id to be
    // configured: the key already knows whose it is.
    let linear = match (&args.linear_secret, &args.linear_key) {
        (Some(secret), Some(key)) => {
            let client = linear::Client::new(key.clone());
            match client.viewer().await {
                Ok((id, name)) => {
                    println!("linear: printing [print] comments by {name}");
                    Some(Linear { client, secret: secret.clone(), viewer_id: id })
                }
                Err(e) => {
                    eprintln!("receipt-server: cannot reach Linear: {e}");
                    std::process::exit(1);
                }
            }
        }
        (None, None) => {
            println!("linear: off (no LINEAR_WEBHOOK_SECRET or LINEAR_API_KEY)");
            None
        }
        _ => {
            eprintln!("receipt-server: Linear needs both the webhook secret and the API key");
            std::process::exit(1);
        }
    };

    let app = Arc::new(App {
        args,
        gate: Mutex::new(Gate {
            day: today(),
            total_today: 0,
            clients: HashMap::new(),
        }),
        printer: tokio::sync::Mutex::new(()),
        linear,
        seen: Mutex::new(VecDeque::new()),
    });

    let routes = Router::new()
        .route("/", get(page))
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/status", get(status))
        .route("/api/print", post(print))
        .route("/linear/webhook", post(linear_webhook))
        .with_state(app);

    let listener = match tokio::net::TcpListener::bind(listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("receipt-server: cannot listen on {listen}: {e}");
            std::process::exit(1);
        }
    };
    println!("listening on http://{listen}");

    axum::serve(
        listener,
        routes.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
    .unwrap();
}

async fn page() -> Html<&'static str> {
    Html(PAGE)
}

async fn status(State(app): State<Arc<App>>) -> Json<Status> {
    let paused = app.args.pause_file.exists();
    let remaining = {
        let mut gate = app.gate.lock().unwrap();
        gate.roll_over();
        match app.args.daily_cap {
            0 => None,
            cap => Some(cap.saturating_sub(gate.total_today)),
        }
    };

    // Opening the device is cheap and tells the truth about cover and paper,
    // which is the whole point of showing a status at all. Ask once: two
    // queries can straddle a cover being closed and contradict each other.
    let (ready, detail) = match tokio::task::spawn_blocking(|| {
        Printer::open().map(|p| p.status())
    })
    .await
    {
        Ok(Ok(problems)) if problems.is_empty() => (true, "ready".to_string()),
        // Say what is wrong either way; only a blocker makes it not ready.
        Ok(Ok(problems)) => {
            let blocked = problems.iter().any(|p| Printer::is_blocking(p));
            (!blocked, problems.join("; "))
        }
        Ok(Err(e)) => (false, e.to_string()),
        Err(e) => (false, e.to_string()),
    };

    Json(Status {
        ready: ready && !paused && remaining != Some(0),
        detail,
        paused,
        remaining_today: remaining,
        max_chars: app.args.max_chars,
    })
}

async fn print(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Submission>,
) -> Result<Json<Reply>, Refusal> {
    if !constant_time_eq(body.password.as_bytes(), app.args.password.as_bytes()) {
        return Err(Refusal::Password);
    }
    if app.args.pause_file.exists() {
        return Err(Refusal::Paused);
    }

    let text = clean(&body.text);
    if text.trim().is_empty() {
        return Err(Refusal::Empty);
    }
    if text.chars().count() > app.args.max_chars {
        return Err(Refusal::TooLong(app.args.max_chars));
    }
    let name = clean(&body.name);
    let name: String = name.trim().chars().take(24).collect();

    // Take the allowance before printing. Someone who is over their limit must
    // not be able to spend paper by racing a second request through.
    app.claim(client_ip(&headers, peer))?;

    let _one_at_a_time = app.printer.lock().await;
    let printed = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut p = Printer::open().map_err(|e| e.to_string())?;
        // Blockers, not warnings: a roll that is merely getting low still has
        // plenty on it, and refusing there would stop printing days early.
        let problems = p.blockers();
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        compose(&mut p, &text, &name);
        p.send().map(|_| ()).map_err(|e| e.to_string())
    })
    .await;

    match printed {
        Ok(Ok(())) => Ok(Json(Reply {
            ok: true,
            message: "printing".into(),
        })),
        Ok(Err(why)) => {
            // The paper was never spent, so hand the allowance back.
            app.refund(client_ip(&headers, peer));
            Err(Refusal::Printer(why))
        }
        Err(e) => {
            app.refund(client_ip(&headers, peer));
            Err(Refusal::Printer(e.to_string()))
        }
    }
}

impl App {
    /// Spend one print from the caller's allowance and from the day's, or say
    /// which limit stopped them.
    fn claim(&self, who: IpAddr) -> Result<(), Refusal> {
        let mut gate = self.gate.lock().unwrap();
        gate.roll_over();

        // Every limit here is off when it is zero, and all of them can be.
        // The password is the real gate; these only stop a loop.
        if self.args.daily_cap > 0 && gate.total_today >= self.args.daily_cap {
            return Err(Refusal::DailyCap);
        }

        let cooldown = Duration::from_secs(self.args.cooldown);
        if let Some(client) = gate.clients.get(&who) {
            let since = client.last.elapsed();
            if self.args.cooldown > 0 && since < cooldown {
                return Err(Refusal::Cooldown((cooldown - since).as_secs() + 1));
            }
            if self.args.per_client_daily > 0 && client.today >= self.args.per_client_daily {
                return Err(Refusal::ClientDaily);
            }
        }

        let client = gate.clients.entry(who).or_insert(Client {
            last: Instant::now(),
            today: 0,
        });
        client.last = Instant::now();
        client.today += 1;
        gate.total_today += 1;
        Ok(())
    }

    /// Give back an allowance spent on a print that never reached paper. The
    /// cooldown stays: a jammed printer should not invite a retry storm.
    fn refund(&self, who: IpAddr) {
        let mut gate = self.gate.lock().unwrap();
        gate.total_today = gate.total_today.saturating_sub(1);
        if let Some(client) = gate.clients.get_mut(&who) {
            client.today = client.today.saturating_sub(1);
        }
    }
}

impl Gate {
    /// Clear the day's counts when the local date changes.
    fn roll_over(&mut self) {
        let now = today();
        if now != self.day {
            self.day = now;
            self.total_today = 0;
            self.clients.clear();
        }
    }
}

/// Days since the epoch in local time — a cheap "which day is it" that does not
/// care about months or leap years.
fn today() -> i32 {
    Local::now().date_naive().num_days_from_ce()
}

/// Who is asking. Behind a tunnel every connection arrives from localhost, so
/// the real address is whatever the proxy put in a header — and only a proxy
/// we control puts one there, since nothing else can reach the port.
fn client_ip(headers: &HeaderMap, peer: SocketAddr) -> IpAddr {
    for header in ["cf-connecting-ip", "x-forwarded-for"] {
        let found = headers
            .get(header)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .and_then(|v| v.trim().parse().ok());
        if let Some(ip) = found {
            return ip;
        }
    }
    peer.ip()
}

/// Strip what the printer cannot render anyway: control characters other than
/// newline, and carriage returns from browsers on Windows. Non-ASCII survives
/// to `encode()`, which maps what cp437 has and turns the rest into '?'.
fn clean(s: &str) -> String {
    s.replace("\r\n", "\n")
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect()
}

/// Lay out one submission: a rule, the message, then who and when in small
/// type. The buffered bytes go out on the caller's `send()`.
fn compose(p: &mut Printer, text: &str, name: &str) {
    p.init();
    p.align(Align::Left).rule(RULE).feed(1);
    p.text_wrapped(text, WIDTH);
    p.feed(1).rule(RULE);

    p.font(Font::B).style(Style { bold: false, ..Style::default() });
    let when = Local::now().format("%a %e %b %H:%M").to_string();
    if name.is_empty() {
        p.align(Align::Right).text(&when);
    } else {
        p.columns(&format!("from {name}"), &when);
    }
    p.align(Align::Left).font(Font::A);

    p.feed(3).cut();
}

/// Compare without leaking how much of the password matched through timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// --- Linear ----------------------------------------------------------------

/// How many recent deliveries to remember, so a retry does not reprint.
const SEEN_LIMIT: usize = 200;

/// Longest slice of an issue description to put on paper.
const DESCRIPTION_CHARS: usize = 400;

/// A comment saying `[print]` arrived.
///
/// Everything here answers 200 unless the caller failed to prove it was Linear.
/// Linear retries a non-200 at one minute, one hour and six hours, and disables
/// the webhook after three failures — so a printer with its cover open must not
/// be allowed to turn the integration off.
async fn linear_webhook(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, String) {
    let Some(cfg) = &app.linear else {
        return (StatusCode::SERVICE_UNAVAILABLE, "linear is not configured".into());
    };

    // Verify before parsing: the signature covers the bytes as sent, and
    // re-serializing the JSON would not reproduce them.
    let signature = headers
        .get("linear-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !linear::signature_matches(&cfg.secret, &body, signature) {
        return (StatusCode::UNAUTHORIZED, "bad signature".into());
    }

    let event: linear::CommentEvent = match serde_json::from_slice(&body) {
        Ok(e) => e,
        // Signed, so it is Linear — just an event shape we do not handle.
        Err(e) => return (StatusCode::OK, format!("ignored: {e}")),
    };

    if !event.is_fresh(chrono::Utc::now().timestamp_millis()) {
        return (StatusCode::UNAUTHORIZED, "stale timestamp".into());
    }

    let verdict = event.verdict(&cfg.viewer_id);
    if !matches!(verdict, linear::Verdict::Print) {
        println!("linear: {}", verdict.describe());
        return (StatusCode::OK, verdict.describe().into());
    }

    // A retry carries the same delivery id, and an edit carries the same
    // comment id; either way it is the same request for the same paper.
    let delivery = headers
        .get("linear-delivery")
        .and_then(|v| v.to_str().ok())
        .unwrap_or(&event.data.id)
        .to_string();
    if app.already_seen(&delivery) || app.already_seen(&event.data.id) {
        println!("linear: already handled {delivery}");
        return (StatusCode::OK, "already handled".into());
    }
    app.remember(delivery);
    app.remember(event.data.id.clone());

    // Answer now, print after. Fetching the issue alone can outlast the five
    // seconds Linear allows.
    let issue_id = event.data.issue_id.clone();
    let note = linear::note(&event.data.body);
    tokio::spawn(async move { fetch_and_print(app, issue_id, note).await });

    (StatusCode::OK, "printing".into())
}

impl App {
    fn already_seen(&self, key: &str) -> bool {
        self.seen.lock().unwrap().iter().any(|k| k == key)
    }

    fn remember(&self, key: String) {
        let mut seen = self.seen.lock().unwrap();
        seen.push_back(key);
        while seen.len() > SEEN_LIMIT {
            seen.pop_front();
        }
    }
}

/// Fetch the issue the comment hangs off, then put it on paper — retrying for
/// a few minutes, because the usual reason this fails is a cover left open.
async fn fetch_and_print(app: Arc<App>, issue_id: String, note: String) {
    let cfg = app.linear.as_ref().expect("checked by the handler");
    let issue = match cfg.client.issue(&issue_id).await {
        Ok(i) => Arc::new(i),
        Err(e) => {
            eprintln!("linear: cannot fetch {issue_id}: {e}");
            return;
        }
    };
    let note = Arc::new(note);

    for attempt in 1..=app.args.linear_retries {
        if app.args.pause_file.exists() {
            println!("linear: paused, dropping {}", issue.identifier);
            return;
        }

        // Named apart from the originals: the closure takes these, and the
        // loop still needs the originals for the next attempt.
        let (job_issue, job_note) = (issue.clone(), note.clone());
        let guard = app.printer.lock().await;
        let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let mut p = Printer::open().map_err(|e| e.to_string())?;
            let problems = p.blockers();
            if !problems.is_empty() {
                return Err(problems.join("; "));
            }
            compose_issue(&mut p, &job_issue, &job_note);
            p.send().map(|_| ()).map_err(|e| e.to_string())
        })
        .await;
        drop(guard);

        match result {
            Ok(Ok(())) => {
                println!("linear: printed {}", issue.identifier);
                return;
            }
            Ok(Err(why)) => {
                eprintln!("linear: {} attempt {attempt}: {why}", issue.identifier);
            }
            Err(e) => eprintln!("linear: {} attempt {attempt}: {e}", issue.identifier),
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
    eprintln!("linear: gave up on {} — comment again to retry", issue.identifier);
}

/// One issue, one receipt.
fn compose_issue(p: &mut Printer, issue: &linear::Issue, note: &str) {
    p.init();
    p.align(Align::Left);
    p.rule(RULE);
    p.columns(&issue.identifier, issue.priority_label.as_deref().unwrap_or(""));
    p.rule(RULE);
    p.feed(1);

    // Double-size needs double line spacing or the rows pull apart.
    p.style(Style::big()).line_spacing(Font::A.height() * 2);
    p.text_wrapped(&issue.title, WIDTH_BIG);
    p.default_spacing().style(Style::default());
    p.feed(1);

    field(p, "State", issue.state.as_ref().map(|s| s.name.clone()));
    field(p, "Assignee", issue.assignee.as_ref().map(|a| a.name.clone()));
    field(p, "Estimate", issue.estimate.map(points));
    field(p, "Due", issue.due_date.clone());
    field(p, "Project", issue.project.as_ref().map(|s| s.name.clone()));
    let labels = issue.label_names().join(", ");
    field(p, "Labels", (!labels.is_empty()).then_some(labels));

    if let Some(description) = issue.description.as_deref()
        && !description.trim().is_empty()
    {
        p.feed(1);
        p.text_wrapped(&truncate(description, DESCRIPTION_CHARS), WIDTH);
    }

    if !note.is_empty() {
        p.feed(1);
        // Quoted, so it reads as the thing you wrote rather than part of the
        // issue.
        for line in note.lines() {
            p.text_wrapped(&format!("> {line}"), WIDTH);
        }
    }

    p.feed(1).rule(RULE);
    p.font(Font::B).align(Align::Centre);
    p.text(&Local::now().format("%H:%M %a %e %b").to_string());
    p.align(Align::Left).font(Font::A);

    p.feed(3).cut();
}

/// `Label      value`, skipped entirely when there is no value — an empty field
/// tells you nothing and costs a line of paper.
fn field(p: &mut Printer, label: &str, value: Option<String>) {
    let Some(value) = value else { return };
    if value.trim().is_empty() {
        return;
    }
    let indent = " ".repeat(11usize.saturating_sub(label.chars().count()));
    p.text_wrapped(&format!("{label}{indent}{value}"), WIDTH);
}

/// Linear returns estimates as a float; 3 points should not read "3.0".
fn points(estimate: f64) -> String {
    if estimate.fract() == 0.0 {
        format!("{estimate:.0}")
    } else {
        format!("{estimate}")
    }
}

/// Cut on a character boundary, and say that it was cut.
fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut = s.char_indices().nth(max).map_or(s.len(), |(i, _)| i);
    format!("{}…", s[..cut].trim_end())
}
