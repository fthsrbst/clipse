/// Password managers whose clipboard writes we suppress by default, even if
/// the platform's own concealment marker (`ExcludeClipboardContentFromMonitorProcessing`,
/// `org.nspasteboard.ConcealedType`, ...) is absent — not every one of these
/// consistently sets it, and app-name matching is the fallback net.
pub const DEFAULT_BLOCKED_APPS: &[&str] = &[
    "1Password",
    "Bitwarden",
    "KeePassXC",
    "LastPass",
    "Dashlane",
    "Proton Pass",
    "Keeper",
    "Enpass",
    "Keychain Access",
];

/// Case-insensitive, punctuation-insensitive app blocklist.
///
/// Matching is substring-based on a normalized form (lowercased, letters and
/// digits only) rather than exact equality, because the same app shows up
/// under different names across platforms and packaging: the process name
/// Windows reports might be `1Password.exe`, the bundle name macOS reports
/// might be `1Password 8`, and a user-configured entry might just be
/// `1password`. Normalizing away case, spaces and punctuation before
/// comparing means all of those match one blocklist entry.
#[derive(Clone, Debug, Default)]
pub struct AppBlocklist {
    normalized: Vec<String>,
}

impl AppBlocklist {
    pub fn new(entries: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            normalized: entries.into_iter().map(|e| normalize(&e.into())).collect(),
        }
    }

    /// Add more entries on top of whatever is already configured — the usual
    /// way to extend the default list with a user's own additions.
    pub fn with_extra(mut self, entries: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.normalized
            .extend(entries.into_iter().map(|e| normalize(&e.into())));
        self
    }

    pub fn is_blocked(&self, app_name: &str) -> bool {
        let app = normalize(app_name);
        !app.is_empty()
            && self
                .normalized
                .iter()
                .any(|entry| app.contains(entry.as_str()))
    }
}

impl FromIterator<String> for AppBlocklist {
    fn from_iter<T: IntoIterator<Item = String>>(iter: T) -> Self {
        Self::new(iter)
    }
}

fn default_blocklist() -> AppBlocklist {
    AppBlocklist::new(DEFAULT_BLOCKED_APPS.iter().copied())
}

impl AppBlocklist {
    /// The sensible-defaults constructor named to be found next to
    /// `std::default::Default` in docs, while keeping `Default::default()`
    /// (an *empty* blocklist, per its usual meaning) unsurprising.
    pub fn defaults() -> Self {
        default_blocklist()
    }
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_list_catches_common_password_managers() {
        let list = AppBlocklist::defaults();
        assert!(list.is_blocked("1Password.exe"));
        assert!(list.is_blocked("1Password 8"));
        assert!(list.is_blocked("Bitwarden.exe"));
        assert!(list.is_blocked("KeePassXC"));
        assert!(list.is_blocked("keepassxc.bin"));
        assert!(list.is_blocked("LastPass"));
        assert!(list.is_blocked("Dashlane"));
        assert!(list.is_blocked("ProtonPass.exe"));
        assert!(list.is_blocked("Proton Pass"));
        assert!(list.is_blocked("Keeper Password Manager"));
        assert!(list.is_blocked("Enpass"));
        assert!(list.is_blocked("Keychain Access"));
    }

    #[test]
    fn matching_is_case_and_punctuation_insensitive() {
        let list = AppBlocklist::new(["1Password"]);
        assert!(list.is_blocked("1PASSWORD.EXE"));
        assert!(list.is_blocked("1-password"));
    }

    #[test]
    fn unrelated_apps_are_not_blocked() {
        let list = AppBlocklist::defaults();
        assert!(!list.is_blocked("notepad.exe"));
        assert!(!list.is_blocked("chrome.exe"));
        assert!(!list.is_blocked(""));
    }

    #[test]
    fn empty_blocklist_blocks_nothing() {
        let list = AppBlocklist::default();
        assert!(!list.is_blocked("1Password.exe"));
    }

    #[test]
    fn custom_entries_extend_rather_than_replace() {
        let list = AppBlocklist::defaults().with_extra(["MyCompanyVault"]);
        assert!(
            list.is_blocked("1Password.exe"),
            "defaults must still apply"
        );
        assert!(
            list.is_blocked("MyCompanyVault.exe"),
            "extra entry must apply"
        );
    }
}
