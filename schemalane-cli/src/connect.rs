use deadpool_postgres::{ManagerConfig, Pool, RecyclingMethod};
use rustls_platform_verifier::ConfigVerifierExt;
use schemalane_core::SchemalaneError;
use tokio_postgres::NoTls;

pub(crate) struct PostgresTarget {
    pub(crate) user: Option<String>,
    pub(crate) host: String,
    pub(crate) port: Option<u16>,
    pub(crate) database: String,
}

pub(crate) fn create_pool(database_url: &str) -> Result<Pool, SchemalaneError> {
    let pg_config: tokio_postgres::Config = database_url.parse().map_err(|error| {
        SchemalaneError::Config(format!("failed to parse database URL: {error}"))
    })?;
    let manager_config = ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    };
    let manager = if wants_tls(&pg_config) {
        let tls_config = rustls::ClientConfig::with_platform_verifier().map_err(|error| {
            SchemalaneError::Config(format!("failed to configure TLS verifier: {error}"))
        })?;
        let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);
        deadpool_postgres::Manager::from_config(pg_config, tls, manager_config)
    } else {
        deadpool_postgres::Manager::from_config(pg_config, NoTls, manager_config)
    };
    Pool::builder(manager).max_size(5).build().map_err(|error| {
        SchemalaneError::Config(format!("failed to build connection pool: {error}"))
    })
}

pub(crate) fn wants_tls(config: &tokio_postgres::Config) -> bool {
    config.get_ssl_mode() != tokio_postgres::config::SslMode::Disable
}

pub(crate) fn format_postgres_target(database_url: &str) -> String {
    match parse_postgres_target(database_url) {
        Some(target) => {
            let user = target
                .user
                .as_deref()
                .map_or_else(String::new, |value| format!("{value}@"));
            let port = target
                .port
                .map_or_else(String::new, |value| format!(":{value}"));
            format!("{user}{}{port}/{}", target.host, target.database)
        }
        None => "<unparsed-url>".to_owned(),
    }
}

pub(crate) fn parse_postgres_target(database_url: &str) -> Option<PostgresTarget> {
    let without_scheme = database_url
        .strip_prefix("postgres://")
        .or_else(|| database_url.strip_prefix("postgresql://"))?;
    let (authority, path) = without_scheme.split_once('/')?;
    let database = path.split(['?', '#']).next()?.to_owned();
    if database.is_empty() {
        return None;
    }
    let (userinfo, hostport) = authority
        .rsplit_once('@')
        .map_or((None, authority), |(user, host)| (Some(user), host));
    let user = userinfo
        .and_then(|raw| raw.split(':').next())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let (host, port) = parse_host_port(hostport)?;
    Some(PostgresTarget {
        user,
        host,
        port,
        database,
    })
}

fn parse_host_port(value: &str) -> Option<(String, Option<u16>)> {
    if let Some(stripped) = value.strip_prefix('[') {
        let (host, rest) = stripped.split_once(']')?;
        if rest.is_empty() {
            return Some((host.to_owned(), None));
        }
        let port = rest
            .strip_prefix(':')
            .and_then(|candidate| candidate.parse::<u16>().ok());
        return Some((host.to_owned(), port));
    }
    if let Some((host, port_text)) = value.rsplit_once(':')
        && !host.is_empty()
        && port_text
            .chars()
            .all(|character| character.is_ascii_digit())
    {
        return Some((host.to_owned(), port_text.parse::<u16>().ok()));
    }
    Some((value.to_owned(), None))
}
