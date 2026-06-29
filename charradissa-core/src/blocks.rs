/// Strip Amassada block-protocol markers before posting a reply to Matrix.
///
/// The block protocol (`[MAIN]`, `[CONSULT]`, `[BTW]`, `[LEAVE]`, etc.) is an
/// internal session-engine convention. Content between `[MAIN]` markers is kept;
/// everything else (sidebars, session-control blocks) is discarded.
/// Text that contains no block markers is returned unchanged.
///
/// Multiple `[MAIN]` blocks within a single response are all kept and concatenated
/// in order (not first-wins). `[MAIN]` anywhere on a line is matched — preamble text
/// on the same line before `[MAIN]` is discarded (model reasoning that leaked before
/// the marker), and any trailing text after `[MAIN]` on the same line is captured. The
/// `[REQUEST_APPROVAL` entry in the discard list intentionally omits the closing
/// `]` so that it matches any variant of the tag as a prefix (e.g.
/// `[REQUEST_APPROVAL reason="..."]`).
pub fn strip_block_markers(text: &str) -> String {
    const DISCARD: &[&str] = &[
        "[LEAVE]",
        "[CONSULT to:",
        "[BTW to:",
        "[INVITE:",
        "[RELEASE:",
        "[FORK_CONSULTATION:",
        "[ADJUST_BUDGET:",
        "[REQUEST_APPROVAL",
        "[MODEL:",
        "[CLOSE]",
    ];

    // Fast path: nothing to strip.
    if !text.contains("[MAIN]") && !DISCARD.iter().any(|m| text.contains(m)) {
        return text.to_string();
    }

    let mut keep = false;
    let mut skip_next_blank = false;
    let mut lines: Vec<&str> = Vec::new();

    for line in text.lines() {
        let t = line.trim();
        if let Some(pos) = t.find("[MAIN]") {
            keep = true;
            skip_next_blank = true;
            let rest = t[pos + "[MAIN]".len()..].trim();
            if !rest.is_empty() {
                lines.push(rest);
                skip_next_blank = false;
            }
            continue;
        }
        if DISCARD.iter().any(|m| t.starts_with(m)) {
            keep = false;
            continue;
        }
        if keep {
            if skip_next_blank && t.is_empty() {
                skip_next_blank = false;
                continue;
            }
            skip_next_blank = false;
            lines.push(line);
        }
    }

    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_when_no_markers() {
        let text = "Hello, I am Guilhem.";
        assert_eq!(strip_block_markers(text), text);
    }

    #[test]
    fn extracts_main_section() {
        let text = "[MAIN]\n\nThis is the reply.\n\n[LEAVE]";
        assert_eq!(strip_block_markers(text), "This is the reply.");
    }

    #[test]
    fn strips_consult_keeps_main() {
        let text = "[CONSULT to: farga]\nsome internal question\n\n[MAIN]\n\nActual reply.\n\n[LEAVE]";
        assert_eq!(strip_block_markers(text), "Actual reply.");
    }

    #[test]
    fn concatenates_multiple_main_blocks() {
        let text = "[MAIN]\n\nFirst part.\n\n[MAIN]\n\nSecond part.";
        assert_eq!(strip_block_markers(text), "First part.\n\nSecond part.");
    }

    #[test]
    fn main_content_on_same_line() {
        let text = "[MAIN] inline reply here";
        assert_eq!(strip_block_markers(text), "inline reply here");
    }

    #[test]
    fn main_preceded_by_preamble_on_same_line() {
        let text = "I have reviewed the context.[MAIN]\n\nActual answer here.\n\n[LEAVE]";
        assert_eq!(strip_block_markers(text), "Actual answer here.");
    }

    #[test]
    fn btw_and_leave_discarded() {
        let text = "[BTW to: room]\nside note\n\n[MAIN]\n\nReal answer.\n\n[LEAVE]\ngoodbye";
        assert_eq!(strip_block_markers(text), "Real answer.");
    }
}
