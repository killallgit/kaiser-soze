/// `https://user:token@host/owner/repo.git?x#y` and `git@host:owner/repo.git` both become `host/owner/repo`.
/// The host is lowercased and a port is dropped. `host:8443/owner/repo` without a scheme is scp-style, not a port.
pub(super) fn project_id_of(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unzip();
    let without_extras = rest.unwrap_or(url).split(['?', '#']).next().unwrap_or_default();
    let host_and_path = without_extras.rsplit('@').next().unwrap_or_default();
    let id = if scheme.is_none() && host_and_path.starts_with(['/', '.']) {
        host_and_path.to_string()
    } else {
        let (host, after_host) = split_host(host_and_path);
        let path = if scheme.is_some() {
            after_host.split_once('/').map_or("", |(_port, path)| path)
        } else {
            after_host.strip_prefix([':', '/']).unwrap_or_default()
        };
        format!("{}/{path}", host.to_lowercase())
    };
    id.trim_end_matches('/').trim_end_matches(".git").to_string()
}

/// Splits `host:port/path`, `host:path` and `[::1]:path` into the host and what follows it.
fn split_host(host_and_path: &str) -> (&str, &str) {
    let host_end = if host_and_path.starts_with('[') {
        host_and_path.find(']').map_or(host_and_path.len(), |close| close + 1)
    } else {
        host_and_path.find([':', '/']).unwrap_or(host_and_path.len())
    };
    host_and_path.split_at(host_end)
}
