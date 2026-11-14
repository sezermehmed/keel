//! Edits `mimeapps.list` without disturbing associations for other programs.

/// `restore` is the previous default for each mime type, in the same order.
/// `None` means there was no default.
pub fn grant(contents: &str, desktop_id: &str, mimes: &[&str]) -> String {
    let mut lines = split_lines(contents);
    for mime in mimes {
        set_key(
            &mut lines,
            "Default Applications",
            mime,
            Some(format!("{desktop_id};")),
        );
        let added = list_value(&lines, "Added Associations", mime);
        let mut items = split_list(added.as_deref().unwrap_or(""));
        if !items.iter().any(|item| item == desktop_id) {
            items.push(desktop_id.to_owned());
        }
        set_key(
            &mut lines,
            "Added Associations",
            mime,
            Some(join_list(&items)),
        );
        let removed = list_value(&lines, "Removed Associations", mime);
        if let Some(current) = removed {
            let kept: Vec<String> = split_list(&current)
                .into_iter()
                .filter(|item| item != desktop_id)
                .collect();
            let value = if kept.is_empty() {
                None
            } else {
                Some(join_list(&kept))
            };
            set_key(&mut lines, "Removed Associations", mime, value);
        }
    }
    strip_empty_sections(&mut lines);
    serialize(&lines)
}

pub fn revoke(
    contents: &str,
    desktop_id: &str,
    mimes: &[&str],
    restore: &[Option<String>],
) -> String {
    let mut lines = split_lines(contents);
    for (index, mime) in mimes.iter().enumerate() {
        let current = list_value(&lines, "Default Applications", mime);
        if let Some(current) = current {
            let items = split_list(&current);
            if items.first().map(String::as_str) == Some(desktop_id) {
                let replacement = restore
                    .get(index)
                    .and_then(|value| value.clone())
                    .map(|previous| format!("{previous};"));
                set_key(&mut lines, "Default Applications", mime, replacement);
            }
        }
        let added = list_value(&lines, "Added Associations", mime);
        if let Some(current) = added {
            let kept: Vec<String> = split_list(&current)
                .into_iter()
                .filter(|item| item != desktop_id)
                .collect();
            let value = if kept.is_empty() {
                None
            } else {
                Some(join_list(&kept))
            };
            set_key(&mut lines, "Added Associations", mime, value);
        }
    }
    strip_empty_sections(&mut lines);
    serialize(&lines)
}

pub fn defaults_for(contents: &str, mimes: &[&str]) -> Vec<Option<String>> {
    let lines = split_lines(contents);
    mimes
        .iter()
        .map(|mime| {
            list_value(&lines, "Default Applications", mime)
                .and_then(|value| split_list(&value).into_iter().next())
        })
        .collect()
}

fn split_lines(contents: &str) -> Vec<String> {
    let normalized = contents.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<String> = normalized.split('\n').map(str::to_owned).collect();
    if lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines
}

fn serialize(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn section_header(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix('[')?.strip_suffix(']')?;
    if inner.is_empty() || inner.contains('[') || inner.contains(']') {
        return None;
    }
    Some(inner.trim())
}

fn section_bounds(lines: &[String], name: &str) -> Option<(usize, usize)> {
    let mut start = None;
    for (index, line) in lines.iter().enumerate() {
        if let Some(found) = section_header(line) {
            if let Some(start) = start {
                return Some((start, index));
            }
            if found == name {
                start = Some(index);
            }
        }
    }
    start.map(|start| (start, lines.len()))
}

fn key_indexes(lines: &[String], start: usize, end: usize, key: &str) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .take(end)
        .skip(start + 1)
        .filter_map(|(index, line)| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            let (name, _) = trimmed.split_once('=')?;
            (name.trim() == key).then_some(index)
        })
        .collect()
}

fn list_value(lines: &[String], section: &str, key: &str) -> Option<String> {
    let (start, end) = section_bounds(lines, section)?;
    let index = key_indexes(lines, start, end, key).into_iter().next()?;
    let (_, value) = lines[index].trim().split_once('=')?;
    Some(value.trim().to_owned())
}

fn set_key(lines: &mut Vec<String>, section: &str, key: &str, value: Option<String>) {
    if let Some((start, end)) = section_bounds(lines, section) {
        let indexes = key_indexes(lines, start, end, key);
        if indexes.is_empty() {
            if let Some(value) = value {
                lines.insert(start + 1, format!("{key}={value}"));
            }
            return;
        }
        match value {
            Some(value) => {
                lines[indexes[0]] = format!("{key}={value}");
                for index in indexes.into_iter().skip(1).rev() {
                    lines.remove(index);
                }
            }
            None => {
                for index in indexes.into_iter().rev() {
                    lines.remove(index);
                }
            }
        }
        return;
    }

    if let Some(value) = value {
        if !lines.is_empty() && lines.last().is_some_and(|line| !line.is_empty()) {
            lines.push(String::new());
        }
        lines.push(format!("[{section}]"));
        lines.push(format!("{key}={value}"));
    }
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

fn join_list(items: &[String]) -> String {
    let mut text = items.join(";");
    if !text.is_empty() {
        text.push(';');
    }
    text
}

fn strip_empty_sections(lines: &mut Vec<String>) {
    let mut index = 0;
    while index < lines.len() {
        if section_header(&lines[index]).is_none() {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        let mut meaningful = false;
        while end < lines.len() && section_header(&lines[end]).is_none() {
            let trimmed = lines[end].trim();
            if !trimmed.is_empty() {
                meaningful = true;
                break;
            }
            end += 1;
        }
        if meaningful {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while end < lines.len() && section_header(&lines[end]).is_none() {
            end += 1;
        }
        lines.drain(index..end);
    }
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIMES: [&str; 2] = ["application/x-bittorrent", "x-scheme-handler/magnet"];

    #[test]
    fn grant_keeps_other_apps_and_is_idempotent() {
        let original = "\
# my comment
[Default Applications]
text/plain = gedit.desktop
application/x-bittorrent=transmission.desktop

[Added Associations]
text/plain=gedit.desktop;
application/x-bittorrent=transmission.desktop;

[Removed Associations]
application/x-bittorrent=keel.desktop;vlc.desktop;
";
        let once = grant(original, "keel.desktop", &MIMES);
        let twice = grant(&once, "keel.desktop", &MIMES);
        assert_eq!(once, twice);
        assert!(once.contains("# my comment\n"));
        assert!(once.contains("text/plain = gedit.desktop\n"));
        assert!(once.contains("text/plain=gedit.desktop;\n"));
        assert!(once.contains("application/x-bittorrent=keel.desktop;\n"));
        assert!(once.contains("x-scheme-handler/magnet=keel.desktop;\n"));
        assert!(once.contains("application/x-bittorrent=transmission.desktop;keel.desktop;\n"));
        assert!(once.contains("x-scheme-handler/magnet=keel.desktop;\n"));
        assert!(once.contains("application/x-bittorrent=vlc.desktop;\n"));
        assert!(!once.contains("keel.desktop;vlc"));
        let defaults = defaults_for(&once, &MIMES);
        assert_eq!(
            defaults,
            vec![Some("keel.desktop".into()), Some("keel.desktop".into())]
        );
    }

    #[test]
    fn revoke_restores_the_previous_default_only_when_we_still_are() {
        let granted = grant("", "keel.desktop", &MIMES);
        let previous = [Some("transmission.desktop".into()), None];
        // The user picked a different magnet handler after install.
        let edited = granted.replace(
            "x-scheme-handler/magnet=keel.desktop;",
            "x-scheme-handler/magnet=firefox.desktop;",
        );
        let revoked = revoke(&edited, "keel.desktop", &MIMES, &previous);
        assert!(revoked.contains("application/x-bittorrent=transmission.desktop;\n"));
        assert!(revoked.contains("x-scheme-handler/magnet=firefox.desktop;\n"));
        assert!(!revoked.contains("keel.desktop"));
    }

    #[test]
    fn empty_file_grant_and_revoke_round_trip() {
        let granted = grant("\r\n", "keel.desktop", &MIMES);
        let revoked = revoke(&granted, "keel.desktop", &MIMES, &[None, None]);
        assert_eq!(revoked, "");
    }
}
