//! Remembers Anthropic thinking blocks (with signatures) for Chat clients.
//!
//! Chat Completions has no place to carry a thinking signature, but Anthropic
//! requires the signed thinking block to be echoed back when a tool-use turn
//! continues with thinking enabled. We key the blocks by the turn's first
//! tool_call id and re-insert them when the client sends that turn back.

use crate::anthropic::ContentBlock;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};

const CAPACITY: usize = 4096;

/// Blocks by tool call id, plus insertion order for eviction.
type Entries = (HashMap<String, Vec<ContentBlock>>, VecDeque<String>);

#[derive(Default)]
pub struct ReasoningCache {
    inner: Mutex<Entries>,
}

impl ReasoningCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put(&self, tool_call_id: &str, blocks: Vec<ContentBlock>) {
        if tool_call_id.is_empty() || blocks.is_empty() {
            return;
        }
        let mut g = self.inner.lock();
        let (map, order) = &mut *g;
        if map.insert(tool_call_id.to_string(), blocks).is_none() {
            order.push_back(tool_call_id.to_string());
        }
        while order.len() > CAPACITY {
            if let Some(old) = order.pop_front() {
                map.remove(&old);
            }
        }
    }

    pub fn get(&self, tool_call_id: &str) -> Option<Vec<ContentBlock>> {
        self.inner.lock().0.get(tool_call_id).cloned()
    }

    /// Thinking/redacted blocks of a message, if it has any worth caching.
    pub fn reasoning_blocks(content: &[ContentBlock]) -> Vec<ContentBlock> {
        content
            .iter()
            .filter(|b| match b {
                ContentBlock::Thinking { signature, .. } => signature
                    .as_deref()
                    .is_some_and(|s| !s.is_empty() && !s.starts_with("bro.")),
                ContentBlock::RedactedThinking { .. } => true,
                _ => false,
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_evict() {
        let c = ReasoningCache::new();
        let b = vec![ContentBlock::Thinking {
            thinking: "t".into(),
            signature: Some("s".into()),
        }];
        c.put("call_1", b.clone());
        assert_eq!(c.get("call_1"), Some(b));
        for i in 0..CAPACITY + 1 {
            c.put(
                &format!("k{i}"),
                vec![ContentBlock::RedactedThinking { data: "d".into() }],
            );
        }
        assert!(c.get("call_1").is_none());
    }
}
