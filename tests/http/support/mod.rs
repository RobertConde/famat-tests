//! The HTTP test harness.
//!
//! [`TestApp::start`] gives each test its own world: a fresh Postgres
//! database, its own key prefix in the Garage bucket, and its own server
//! process (the real `famat` binary) on a free port. Tests act and assert
//! over HTTP only; the fixtures in [`fixtures`] write straight to Postgres and
//! Garage to set the scene, because the Admin and import flows that would
//! normally do it come in later tickets.
//!
//! Configuration comes from the same variables the server reads, normally
//! loaded from `scripts/dev-up.sh`'s `dev.env` by `scripts/test.sh`:
//! `DATABASE_URL` (a role allowed to create databases) and `S3_ENDPOINT`,
//! `S3_REGION`, `S3_BUCKET`, `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY`.
//!
//! Optional:
//! - `FAMAT_TEST_SERVER`: a server binary to run instead of the one cargo
//!   built, e.g. a `dx bundle` output with its `public/` directory beside it.
//! - `FAMAT_TEST_KEEP=1`: keep each test's database, objects and server log.

pub mod download;
pub mod fixtures;
pub mod html;

use std::{
    fs::File,
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region, RequestChecksumCalculation};
use sqlx::{Connection, PgConnection, PgPool, postgres::PgPoolOptions};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

/// Settings shared by every test, from the environment.
#[derive(Clone)]
struct Env {
    database_url: String,
    s3_endpoint: String,
    s3_region: String,
    s3_bucket: String,
    s3_access_key_id: String,
    s3_secret_access_key: String,
}

impl Env {
    fn load() -> Self {
        let var = |name: &str| {
            std::env::var(name).unwrap_or_else(|_| {
                panic!(
                    "{name} is not set. Start Postgres and Garage with scripts/dev-up.sh, \
                     then run the tests with scripts/test.sh."
                )
            })
        };
        Self {
            database_url: var("DATABASE_URL"),
            s3_endpoint: var("S3_ENDPOINT"),
            s3_region: var("S3_REGION"),
            s3_bucket: var("S3_BUCKET"),
            s3_access_key_id: var("S3_ACCESS_KEY_ID"),
            s3_secret_access_key: var("S3_SECRET_ACCESS_KEY"),
        }
    }

    fn s3_client(&self) -> aws_sdk_s3::Client {
        let credentials = Credentials::new(
            &self.s3_access_key_id,
            &self.s3_secret_access_key,
            None,
            None,
            "famat-tests",
        );
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(self.s3_region.clone()))
            .endpoint_url(&self.s3_endpoint)
            .credentials_provider(credentials)
            .force_path_style(true)
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .build();
        aws_sdk_s3::Client::from_conf(config)
    }

    /// `DATABASE_URL` with its database name swapped for `name`.
    fn database_url_for(&self, name: &str) -> String {
        let (without_query, query) = match self.database_url.split_once('?') {
            Some((url, query)) => (url, Some(query)),
            None => (self.database_url.as_str(), None),
        };
        let (server, _) = without_query
            .rsplit_once('/')
            .filter(|(server, _)| server.contains("://"))
            .expect("DATABASE_URL must look like postgres://user:pass@host:port/db");
        match query {
            Some(query) => format!("{server}/{name}?{query}"),
            None => format!("{server}/{name}"),
        }
    }
}

/// A running server with its own database and object prefix. Stopped and
/// cleaned up on drop.
pub struct TestApp {
    /// e.g. `http://127.0.0.1:54321`
    pub base_url: String,
    pub http: reqwest::Client,
    /// This test's database, for fixtures.
    pub db: PgPool,
    pub s3: aws_sdk_s3::Client,
    pub bucket: String,
    /// This test's key prefix in the bucket, e.g. `tests/3f2a…/`.
    pub key_prefix: String,
    env: Env,
    database: String,
    server: Child,
    log_path: PathBuf,
}

impl TestApp {
    pub async fn start() -> Self {
        let env = Env::load();
        let id = uuid::Uuid::new_v4().simple().to_string();
        let database = format!("famat_test_{id}");
        let key_prefix = format!("tests/{id}/");

        let mut admin = PgConnection::connect(&env.database_url)
            .await
            .expect("connect to DATABASE_URL to create the test database");
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{database}\"")))
            .execute(&mut admin)
            .await
            .expect("create the test database");
        admin.close().await.ok();
        let database_url = env.database_url_for(&database);

        let log_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("http-tests");
        std::fs::create_dir_all(&log_dir).unwrap();
        let log_path = log_dir.join(format!("{id}.log"));
        let log = File::create(&log_path).unwrap();

        let port = free_port();
        let mut command = match std::env::var_os("FAMAT_TEST_SERVER") {
            Some(binary) => Command::new(binary),
            None => {
                // A cargo-built server has no dx bundle beside it; Dioxus needs
                // some public directory, and the pages render without one.
                let public = log_dir.join("empty-public");
                std::fs::create_dir_all(&public).unwrap();
                let mut command = Command::new(env!("CARGO_BIN_EXE_famat"));
                command.env("DIOXUS_PUBLIC_PATH", public);
                command
            }
        };
        let server = command
            .env("IP", "127.0.0.1")
            .env("PORT", port.to_string())
            .env("DATABASE_URL", &database_url)
            .env("DATABASE_MAX_CONNECTIONS", "4")
            .env("S3_ENDPOINT", &env.s3_endpoint)
            .env("S3_REGION", &env.s3_region)
            .env("S3_BUCKET", &env.s3_bucket)
            .env("S3_ACCESS_KEY_ID", &env.s3_access_key_id)
            .env("S3_SECRET_ACCESS_KEY", &env.s3_secret_access_key)
            .env("S3_KEY_PREFIX", &key_prefix)
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("start the famat server");

        let mut app = TestApp {
            base_url: format!("http://127.0.0.1:{port}"),
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            // Placeholder until the server has run the migrations.
            db: PgPoolOptions::new().connect_lazy(&database_url).unwrap(),
            s3: env.s3_client(),
            bucket: env.s3_bucket.clone(),
            key_prefix,
            env,
            database,
            server,
            log_path,
        };
        app.wait_until_ready().await;
        app.db = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .expect("connect to the test database");
        app
    }

    async fn wait_until_ready(&mut self) {
        let started = Instant::now();
        loop {
            if let Some(status) = self.server.try_wait().unwrap() {
                panic!("the server exited during startup ({status}):\n{}", self.log_tail());
            }
            if let Ok(response) = self.http.get(self.url("/healthz")).send().await
                && response.status().is_success()
            {
                return;
            }
            if started.elapsed() > STARTUP_TIMEOUT {
                panic!("the server did not become ready:\n{}", self.log_tail());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// GET a path as a browser with JavaScript off would: one plain request,
    /// nothing executed.
    pub async fn get(&self, path: &str) -> reqwest::Response {
        self.http.get(self.url(path)).send().await.expect("GET request")
    }

    /// GET a page and parse it, asserting a 200 HTML response.
    pub async fn get_page(&self, path: &str) -> html::Page {
        let response = self.get(path).await;
        assert_eq!(response.status(), 200, "GET {path}");
        let content_type = response.headers()["content-type"].to_str().unwrap().to_owned();
        assert!(
            content_type.starts_with("text/html"),
            "GET {path}: content-type {content_type}"
        );
        html::Page::parse(&response.text().await.unwrap())
    }

    fn log_tail(&self) -> String {
        let log = std::fs::read_to_string(&self.log_path).unwrap_or_default();
        let lines: Vec<&str> = log.lines().collect();
        let tail = lines[lines.len().saturating_sub(40)..].join("\n");
        format!("--- last lines of {} ---\n{tail}", self.log_path.display())
    }
}

impl Drop for TestApp {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
        if std::thread::panicking() {
            eprintln!("{}", self.log_tail());
        }
        if std::env::var_os("FAMAT_TEST_KEEP").is_some() {
            eprintln!(
                "kept database {}, objects under {}, log {}",
                self.database,
                self.key_prefix,
                self.log_path.display()
            );
            return;
        }

        // The test's own runtime is blocked inside this drop, so clean up on
        // a fresh thread with a fresh runtime and fresh connections.
        let env = self.env.clone();
        let database = self.database.clone();
        let prefix = self.key_prefix.clone();
        let log_path = (!std::thread::panicking()).then(|| self.log_path.clone());
        let cleanup = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                if let Ok(mut admin) = PgConnection::connect(&env.database_url).await {
                    let sql = format!("DROP DATABASE IF EXISTS \"{database}\" WITH (FORCE)");
                    let _ = sqlx::query(sqlx::AssertSqlSafe(sql)).execute(&mut admin).await;
                }
                let s3 = env.s3_client();
                let mut pages = s3
                    .list_objects_v2()
                    .bucket(&env.s3_bucket)
                    .prefix(&prefix)
                    .into_paginator()
                    .send();
                while let Some(Ok(page)) = pages.next().await {
                    for object in page.contents() {
                        if let Some(key) = object.key() {
                            let _ = s3.delete_object().bucket(&env.s3_bucket).key(key).send().await;
                        }
                    }
                }
            });
            if let Some(log_path) = log_path {
                let _ = std::fs::remove_file(log_path);
            }
        });
        let _ = cleanup.join();
    }
}

/// A port nothing is listening on right now.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}
