import sys

with open("src/sandbox/mod.rs", "r") as f:
    content = f.read()

content = content.replace(
    "// DNS-level filtering via LD_PRELOAD: only allowed domains can resolve",
    "// Domain allowlist via rune-net-guard (Seccomp USER_NOTIF)"
)
content = content.replace(
    "\"dns-filter({})\".to_string()",
    "\"net-guard({})\".to_string()"
)
content = content.replace(
    "\"dns-filter({})\".to_string(),",
    "\"net-guard({})\".to_string(),"
)
content = content.replace(
    "\"sandbox: DNS filter (LD_PRELOAD)\"",
    "\"sandbox: net-guard active\""
)
content = content.replace(
    "// Full isolation: no network at all (empty allowed_domains = block all)",
    ""
)
content = content.replace(
    "// Layer 3: Seccomp filter (block dangerous syscalls)\n        // We use a pre-exec approach: write a small seccomp filter script",
    "// Layer 3: Seccomp filter via rune-seccomp"
)

# find "        // Build the final command"
idx = content.find("        // Build the final command")

net_guard = """        // Network guard layer
        let mut net_guard_wrapper = None;
        if !self.config.allowed_domains.is_empty() && !self.config.allowed_domains.iter().any(|d| d == "*") {
            let has_net_guard = probe_tool("rune-net-guard").await;
                
            if has_net_guard {
                let domains = self.config.allowed_domains.join(",");
                net_guard_wrapper = Some(format!("rune-net-guard --allow-domains {} --", domains));
            } else {
                warn!("sandbox: rune-net-guard not found, network filtering unavailable");
            }
        }

"""

content = content[:idx] + net_guard + content[idx:]

# replace inner_cmd
inner_cmd_old = """        // Chain both landlock and seccomp when available:
        // landlock ... -- seccomp ... -- sh -c "cmd"
        let inner_cmd = match (&landlock_wrapper, &seccomp_wrapper) {
            (Some(lw), Some(sw)) => {
                // Landlock outer, seccomp inner, then the actual command
                format!("{} {} sh -c {}", lw, sw, shell_escape(cmd))
            }
            (None, Some(sw)) => {
                format!("{} sh -c {}", sw, shell_escape(cmd))
            }
            (Some(lw), None) => {
                format!("{} sh -c {}", lw, shell_escape(cmd))
            }
            (None, None) => {
                format!("sh -c {}", shell_escape(cmd))
            }
        };"""

inner_cmd_new = """        // Chain wrappers: landlock -> seccomp -> net-guard -> sh -c "cmd"
        let mut inner_cmd_parts = Vec::new();
        
        if let Some(lw) = landlock_wrapper {
            inner_cmd_parts.push(lw);
        }
        if let Some(sw) = seccomp_wrapper {
            inner_cmd_parts.push(sw);
        }
        if let Some(ng) = net_guard_wrapper {
            inner_cmd_parts.push(ng);
        }
        
        inner_cmd_parts.push(format!("sh -c {}", shell_escape(cmd)));
        
        let inner_cmd = inner_cmd_parts.join(" ");"""

content = content.replace(inner_cmd_old, inner_cmd_new)

# replace inner_cmd (where it says inner_cmd)
content = content.replace("            inner_cmd\n        } else {\n            format!(\"{} {}\", wrapper_parts.join(\" \"), inner_cmd)\n        };", "            inner_cmd.clone()\n        } else {\n            format!(\"{} {}\", wrapper_parts.join(\" \"), inner_cmd)\n        };")

# remove dns filter block
dns_filter_block = """        // DNS filter: inject LD_PRELOAD + RUNE_ALLOWED_DOMAINS for domain-level network control
        if !self.config.allowed_domains.is_empty()
            && !self.config.allowed_domains.iter().any(|d| d == "*")
        {
            let dns_filter_path = Self::find_dns_filter_lib();
            if let Some(lib_path) = dns_filter_path {
                let domains = self.config.allowed_domains.join(",");
                command.env("LD_PRELOAD", &lib_path);
                command.env("RUNE_ALLOWED_DOMAINS", &domains);
                info!(lib = %lib_path, domains = %domains, "sandbox: DNS filter injected");
            } else {
                warn!("sandbox: librune_dns_filter.so not found, DNS filtering unavailable");
            }
        }"""

content = content.replace(dns_filter_block, "")

with open("src/sandbox/mod.rs", "w") as f:
    f.write(content)
