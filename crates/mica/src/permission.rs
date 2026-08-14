use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Access {
    Read,
    Write,
    Connect,
    Browse,
    List,
    Spawn,
    Kill,
    Stats,
    Random,
    Window,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Resource {
    Path(String),
    HostPort { host: String, port: u16 },
    Program(String),
    Unscoped,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Permission {
    pub namespace: String,
    pub access: Access,
    pub resource: Resource,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PermissionSet {
    entries: Vec<Permission>,
}

impl PermissionSet {
    pub fn parse_manifest(source: &str) -> Result<Self, &'static str> {
        let mut output = Self::default();
        let mut version = false;
        for line in source.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line == "--!mica 1" {
                version = true;
                continue;
            }
            let Some(rule) = line.strip_prefix("--!allow ") else {
                if line.starts_with("--!") {
                    return Err("unknown Mica directive");
                }
                if !line.starts_with("--") {
                    break;
                }
                continue;
            };
            output.insert(Permission::parse(rule.trim())?);
        }
        if !version {
            return Err("missing --!mica 1 directive");
        }
        Ok(output)
    }

    pub fn from_rules<'a>(rules: impl IntoIterator<Item = &'a str>) -> Result<Self, &'static str> {
        let mut output = Self::default();
        for rule in rules {
            output.insert(Permission::parse(rule)?);
        }
        Ok(output)
    }

    pub fn insert(&mut self, permission: Permission) {
        if !self.entries.contains(&permission) {
            self.entries.push(permission);
        }
    }

    pub fn intersect(&self, other: &Self) -> Self {
        let mut output = Self::default();
        for permission in &self.entries {
            if other.allows_permission(permission) {
                output.insert(permission.clone());
            }
        }
        output
    }

    pub fn allows_path(&self, access: Access, path: &str) -> bool {
        let Ok(path) = normalize_path(path) else {
            return false;
        };
        self.entries.iter().any(|permission| {
            permission.namespace == "fs"
                && permission.access == access
                && matches!(&permission.resource, Resource::Path(prefix) if path_prefix(prefix, &path))
        })
    }

    pub fn allows_host(&self, host: &str, port: u16) -> bool {
        self.entries.iter().any(|permission| {
            permission.namespace == "net"
                && permission.access == Access::Connect
                && matches!(&permission.resource, Resource::HostPort { host: allowed, port: allowed_port } if allowed.eq_ignore_ascii_case(host) && *allowed_port == port)
        })
    }

    pub fn allows_host_any_port(&self, host: &str) -> bool {
        self.entries.iter().any(|permission| {
            permission.namespace == "net"
                && permission.access == Access::Connect
                && matches!(&permission.resource, Resource::HostPort { host: allowed, .. } if allowed.eq_ignore_ascii_case(host))
        })
    }

    pub fn allows_unscoped(&self, namespace: &str, access: Access) -> bool {
        self.entries.iter().any(|permission| {
            permission.namespace == namespace
                && permission.access == access
                && permission.resource == Resource::Unscoped
        })
    }

    pub fn allows_program(&self, access: Access, program: &str) -> bool {
        self.entries.iter().any(|permission| {
            permission.namespace == "proc"
                && permission.access == access
                && matches!(&permission.resource, Resource::Program(allowed) if allowed == program)
        })
    }

    pub fn entries(&self) -> &[Permission] {
        &self.entries
    }

    fn allows_permission(&self, requested: &Permission) -> bool {
        match &requested.resource {
            Resource::Path(path) => self.entries.iter().any(|candidate| {
                candidate.namespace == requested.namespace
                    && candidate.access == requested.access
                    && matches!(&candidate.resource, Resource::Path(prefix) if path_prefix(prefix, path))
            }),
            _ => self.entries.contains(requested),
        }
    }
}

impl Permission {
    pub fn parse(rule: &str) -> Result<Self, &'static str> {
        let (name, resource) = rule.split_once(':').unwrap_or((rule, ""));
        let (namespace, operation) = name.split_once('.').unwrap_or((name, ""));
        let access = match (namespace, operation) {
            ("fs", "read") => Access::Read,
            ("fs", "write") => Access::Write,
            ("net", "connect") => Access::Connect,
            ("net", "browse") => Access::Browse,
            ("proc", "list") => Access::List,
            ("proc", "spawn") => Access::Spawn,
            ("proc", "kill") => Access::Kill,
            ("sys", "stats") => Access::Stats,
            ("random", "read") | ("random", "") => Access::Random,
            ("gui", "window") => Access::Window,
            _ => return Err("unknown permission"),
        };
        let resource = match (namespace, access) {
            ("fs", Access::Read | Access::Write) => Resource::Path(normalize_path(resource)?),
            ("net", Access::Connect) => {
                let (host, port) = resource
                    .rsplit_once(':')
                    .ok_or("network rule requires host:port")?;
                if host.is_empty() || host.contains('*') {
                    return Err("network host must be exact");
                }
                Resource::HostPort {
                    host: host.to_ascii_lowercase(),
                    port: port.parse().map_err(|_| "invalid network port")?,
                }
            }
            ("net", Access::Browse) if resource.is_empty() => Resource::Unscoped,
            ("proc", Access::Spawn | Access::Kill) => {
                if resource.is_empty() {
                    return Err("process rule requires program name");
                }
                Resource::Program(resource.to_string())
            }
            _ if resource.is_empty() => Resource::Unscoped,
            _ => return Err("permission does not take a resource"),
        };
        Ok(Self {
            namespace: namespace.to_string(),
            access,
            resource,
        })
    }

    pub fn rule(&self) -> String {
        let operation = match self.access {
            Access::Read => "read",
            Access::Write => "write",
            Access::Connect => "connect",
            Access::Browse => "browse",
            Access::List => "list",
            Access::Spawn => "spawn",
            Access::Kill => "kill",
            Access::Stats => "stats",
            Access::Random => "",
            Access::Window => "window",
        };
        let mut output = if operation.is_empty() {
            self.namespace.clone()
        } else {
            alloc::format!("{}.{}", self.namespace, operation)
        };
        match &self.resource {
            Resource::Path(path) | Resource::Program(path) => {
                output.push(':');
                output.push_str(path);
            }
            Resource::HostPort { host, port } => {
                output.push(':');
                output.push_str(host);
                output.push(':');
                output.push_str(&port.to_string());
            }
            Resource::Unscoped => {}
        }
        output
    }
}

pub fn normalize_path(path: &str) -> Result<String, &'static str> {
    if !path.starts_with('/') || path.as_bytes().contains(&0) {
        return Err("path must be absolute");
    }
    let mut components: Vec<&str> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if components.pop().is_none() {
                    return Err("path escapes root");
                }
            }
            value => components.push(value),
        }
    }
    let mut output = String::from("/");
    output.push_str(&components.join("/"));
    Ok(output)
}

fn path_prefix(prefix: &str, path: &str) -> bool {
    prefix == "/"
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}
