//! Minimal ~/.ssh/config parser used only for the first-run import.
//! System ssh still reads the real config at connect time, so unknown
//! directives (ProxyJump, IdentityAgent, ...) are safely ignored here.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::model::Connection;

#[derive(Debug, Default, Clone)]
pub struct SshHost {
    pub alias: String,
    pub host_name: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
}

pub fn parse(content: &str) -> Vec<SshHost> {
    let mut hosts: Vec<SshHost> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut skipping = false; // inside a Match block

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((keyword, value)) = split_directive(line) else {
            continue;
        };
        match keyword.to_ascii_lowercase().as_str() {
            "host" => {
                skipping = false;
                current.clear();
                for pattern in value.split_whitespace() {
                    let pattern = strip_quotes(pattern);
                    if pattern.contains('*') || pattern.contains('?') || pattern.starts_with('!') {
                        continue;
                    }
                    hosts.push(SshHost {
                        alias: pattern.to_string(),
                        ..SshHost::default()
                    });
                    current.push(hosts.len() - 1);
                }
            }
            "match" => {
                skipping = true;
                current.clear();
            }
            _ if skipping => {}
            "hostname" => {
                for &i in &current {
                    hosts[i].host_name.get_or_insert_with(|| value.clone());
                }
            }
            "user" => {
                for &i in &current {
                    hosts[i].user.get_or_insert_with(|| value.clone());
                }
            }
            "port" => {
                if let Ok(port) = value.parse::<u16>() {
                    for &i in &current {
                        hosts[i].port.get_or_insert(port);
                    }
                }
            }
            "identityfile" => {
                for &i in &current {
                    hosts[i].identity_file.get_or_insert_with(|| value.clone());
                }
            }
            _ => {}
        }
    }
    hosts
}

pub fn to_connections(hosts: Vec<SshHost>, home: &Path) -> Vec<Connection> {
    let mut seen = HashSet::new();
    hosts
        .into_iter()
        .filter(|h| seen.insert(h.alias.clone()))
        .map(|h| Connection {
            host: h.host_name.unwrap_or_else(|| h.alias.clone()),
            name: h.alias,
            port: h.port.unwrap_or(22),
            user: h.user,
            identity_file: h.identity_file.map(|p| expand_tilde(&p, home)),
            sftp_dir: None,
            favorite: false,
            last_used: None,
        })
        .collect()
}

pub fn expand_tilde(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        home.to_path_buf()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(path)
    }
}

/// Split `Keyword value`, `Keyword=value` or `Keyword = value`.
fn split_directive(line: &str) -> Option<(&str, String)> {
    let idx = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let keyword = &line[..idx];
    let rest = line[idx..]
        .trim_start_matches(|c: char| c.is_whitespace() || c == '=')
        .trim();
    if keyword.is_empty() || rest.is_empty() {
        return None;
    }
    Some((keyword, strip_quotes(rest).to_string()))
}

fn strip_quotes(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
# global defaults
Host *
	IdentityAgent "~/Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock"

Host web
	HostName 10.1.2.3
	User deploy
	Port 2222
	IdentityFile ~/.ssh/web-key
	IdentityFile ~/.ssh/second-key-ignored

Host 195.201.16.40
	User root

Host alpha beta gam*ma
	HostName shared.example.com
	User shared

Match user root
	Port 9999

Host quoted
	IdentityFile "~/chaves/my key.pem"

Host equals=host
"#;

    #[test]
    fn parses_fixture() {
        let hosts = parse(FIXTURE);
        let aliases: Vec<&str> = hosts.iter().map(|h| h.alias.as_str()).collect();
        // `Host *` and `gam*ma` skipped; Match block ignored entirely.
        assert_eq!(
            aliases,
            vec!["web", "195.201.16.40", "alpha", "beta", "quoted", "equals=host"]
        );

        let web = &hosts[0];
        assert_eq!(web.host_name.as_deref(), Some("10.1.2.3"));
        assert_eq!(web.user.as_deref(), Some("deploy"));
        assert_eq!(web.port, Some(2222));
        // first IdentityFile wins
        assert_eq!(web.identity_file.as_deref(), Some("~/.ssh/web-key"));

        // Match block must not leak Port into following state
        assert_eq!(hosts[1].port, None);

        // multi-pattern hosts share directives
        assert_eq!(hosts[2].host_name.as_deref(), Some("shared.example.com"));
        assert_eq!(hosts[3].host_name.as_deref(), Some("shared.example.com"));

        // quoted value stripped
        assert_eq!(hosts[4].identity_file.as_deref(), Some("~/chaves/my key.pem"));
    }

    #[test]
    fn hostname_defaults_to_alias_and_tilde_expands() {
        let hosts = parse("Host bare\n\tUser me\n\tIdentityFile ~/.ssh/k\n");
        let conns = to_connections(hosts, Path::new("/home/me"));
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].name, "bare");
        assert_eq!(conns[0].host, "bare");
        assert_eq!(conns[0].port, 22);
        assert_eq!(
            conns[0].identity_file.as_deref(),
            Some(Path::new("/home/me/.ssh/k"))
        );
    }

    #[test]
    fn duplicate_aliases_keep_first() {
        let hosts = parse("Host dup\n\tHostName first\nHost dup\n\tHostName second\n");
        let conns = to_connections(hosts, Path::new("/home/me"));
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].host, "first");
    }

    #[test]
    fn empty_or_garbage_config() {
        assert!(parse("").is_empty());
        assert!(parse("# only comments\n\n").is_empty());
        assert!(parse("Host *\n\tUser x\n").is_empty());
    }
}
