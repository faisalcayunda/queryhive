//! The `*`/`?` matcher shared by `known_hosts` host patterns and `ssh_config` `Host` lines.

/// `*` and `?`, case already folded, matching OpenSSH's `match_pattern` (which supports
/// nothing else — no character classes).
///
/// Iterative rather than recursive: a pattern full of `*` against a long hostname is
/// exponential work for the naive version, and this input is a file on disk.
pub(crate) fn glob(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let (mut p, mut t) = (0, 0);
    let mut star: Option<usize> = None;
    let mut resume = 0;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            resume = t;
            p += 1;
        } else if let Some(star) = star {
            p = star + 1;
            resume += 1;
            t = resume;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}
