/// Extract all ```mermaid ... ``` code blocks from a Matrix message body.
pub fn extract_mermaid_blocks(content: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let open = "```mermaid";
    let close = "```";
    let mut rest = content;
    while let Some(start) = rest.find(open) {
        let after_open = &rest[start + open.len()..];
        let body = after_open.trim_start_matches('\n').trim_start_matches('\r');
        if let Some(end) = body.find(close) {
            let diagram = body[..end].trim();
            if !diagram.is_empty() {
                blocks.push(diagram.to_string());
            }
            rest = &body[end + close.len()..];
        } else {
            break;
        }
    }
    blocks
}

/// POST the diagram source to Kroki and return the rendered SVG bytes.
pub async fn render_svg(kroki_url: &str, diagram: &str) -> anyhow::Result<Vec<u8>> {
    let url = format!("{}/mermaid/svg", kroki_url);
    let resp = reqwest::Client::new()
        .post(&url)
        .header("Content-Type", "text/plain")
        .body(diagram.to_string())
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Kroki request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Kroki returned {}: {}", status, body);
    }

    Ok(resp.bytes().await.map(|b| b.to_vec())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_single_block() {
        let msg = "look at this\n```mermaid\ngraph TD\n  A-->B\n```\ncool right?";
        let blocks = extract_mermaid_blocks(msg);
        assert_eq!(blocks, vec!["graph TD\n  A-->B"]);
    }

    #[test]
    fn extracts_multiple_blocks() {
        let msg = "```mermaid\nflowchart LR\n  A-->B\n```\nand\n```mermaid\nsequenceDiagram\n  A->>B: hi\n```";
        let blocks = extract_mermaid_blocks(msg);
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn ignores_non_mermaid_blocks() {
        let msg = "```rust\nfn main() {}\n```";
        assert!(extract_mermaid_blocks(msg).is_empty());
    }

    #[test]
    fn empty_message() {
        assert!(extract_mermaid_blocks("hello world").is_empty());
    }
}
