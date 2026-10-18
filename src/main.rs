mod backup;
mod bots;
mod clock;
mod condition;
mod config;
mod content;
mod crypto;
mod dungeon;
mod encounter;
mod endgame;
mod loot;
mod performance;
mod quest;
mod server;
mod store;
mod terrain;
mod wire;
mod world;
use anyhow::{Result, ensure};
use rand::RngCore;
use std::sync::Arc;
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if terrain::prepare::run(&args)? {
        return Ok(());
    }
    serve(args)
}
#[tokio::main]
async fn serve(args: Vec<String>) -> Result<()> {
    if backup::run(&args).await? {
        return Ok(());
    }
    let started = std::time::Instant::now();
    let filter = match std::env::var("RUST_LOG") {
        Ok(v) => tracing_subscriber::EnvFilter::try_new(v)?,
        Err(std::env::VarError::NotPresent) => tracing_subscriber::EnvFilter::new("openrotmg=info"),
        Err(e) => return Err(e.into()),
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .init();
    let (config, dev_control) = config::Config::load(&args)?;
    let dialogue = bots::dialogue::Service::configured(config.bot_llm)?;
    let client = config.client;
    let state = config.state;
    tracing::info!(address=%config.bind, state=%state.display(), client=%client.display(), data=%config.data.display(), "local environment selected");
    std::fs::create_dir_all(&state)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700))?;
    }
    for file in ["index.html", "vendor/ruffle.js"] {
        ensure!(
            client.join(file).is_file(),
            "missing client asset {file}; run ./run.sh to download and prepare the local client"
        );
    }
    ensure!(
        client.join("client.swf").is_file(),
        "missing reference client {}; run ./run.sh to download the pinned client",
        client.join("client.swf").display()
    );
    let content = Arc::new(content::Content::load(&config.data)?);
    let keys = Arc::new(crypto::Keys::load(&state)?);
    let swf = Arc::new(keys.client(&client.join("client.swf"))?);
    if config.test_state {
        ensure!(
            !state.join("game.db").exists(),
            "--test-state refuses an existing game database; resume an owned prepared save with --state and --dev-control"
        );
        use std::io::Write;
        let mut marker = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(state.join("testing.json"))?;
        marker.write_all(br#"{"schema":1,"provenance":"prepared"}"#)?;
        marker.sync_all()?;
    }
    let testing = server::testing::Testing::open(&state)?;
    ensure!(
        testing.is_none() || dev_control,
        "prepared test state requires --dev-control"
    );
    ensure!(
        testing.is_none() || !config.shore_bots,
        "prepared sessions require --no-shore-bots or shore_bots:false; spawn every controller explicitly"
    );
    let store = store::Store::open(&state.join("game.db"), &content)?;
    let directory = match &testing {
        Some(_) => world::Directory::prepared(
            &config.data,
            content.clone(),
            config.max_players,
            config.realm_seed,
        )?,
        None => world::Directory::with_seed(
            &config.data,
            content.clone(),
            config.max_players,
            config.realm_seed,
        )?,
    };
    if testing.is_some() {
        directory.clock.set_paused(true);
    }
    let world = world::Handle::spawn(directory);
    let control = if dev_control {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let token = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let path = state.join("control.token");
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        use std::io::Write;
        opts.open(&path)?.write_all(token.as_bytes())?;
        tracing::info!(token_file=%path.display(),"loopback script control enabled");
        Some(token)
    } else {
        None
    };
    let app = server::App {
        started,
        testing,
        diagnostics: Arc::new(server::diagnostics::Diagnostics::open(
            state.join("bug-reports"),
        )?),
        store,
        world,
        keys,
        content,
        swf,
        origin: config.origin,
        sfx: config.data.join("sfx"),
        max_players: config.max_players,
        behind_proxy: config.behind_proxy,
        shore_bots: config.shore_bots,
        dialogue,
        society: Arc::default(),
        requests: Arc::default(),
        connections: Arc::new(tokio::sync::Semaphore::new(config.max_players * 2)),
        control,
        discord_link_token: match std::env::var("DISCORD_LINK_TOKEN_FILE") {
            Ok(path) => {
                let token = std::fs::read_to_string(path)?;
                ensure!(
                    token.trim().len() == 64 && token.trim().bytes().all(|b| b.is_ascii_hexdigit()),
                    "invalid Discord link API credential"
                );
                Some(token.trim().to_string())
            }
            Err(std::env::VarError::NotPresent) => None,
            Err(error) => return Err(error.into()),
        },
        owners: Arc::default(),
        authentication: Arc::default(),
        guild_actions: Arc::default(),
        gold_updates: Arc::default(),
        bots: Arc::default(),
        quarantined: Arc::default(),
        shutdown: tokio::sync::watch::channel(false).0,
    };
    server::serve(app, config.bind, client).await
}
