//! Local omnibox suggestions (no remote suggestion services).

use crate::bookmarks_repo::Bookmark;
use crate::history_repo::{self, HistoryRecord, VisitTransition};
use crate::search::SearchProvider;
use crate::time_util::now_ms;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionKind {
    History,
    Bookmark,
    OpenTab,
    Search,
    Url,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionSource {
    LocalHistory,
    LocalBookmark,
    OpenTabs,
    SearchProvider,
    TypedUrl,
}

#[derive(Debug, Clone)]
pub struct Suggestion {
    pub kind: SuggestionKind,
    pub source: SuggestionSource,
    pub title: String,
    pub url: String,
    pub score: i32,
    /// When set, activating switches to this tab index instead of navigating.
    pub open_tab_index: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct SuggestionModel {
    pub items: Vec<Suggestion>,
    pub selected: Option<usize>,
}

impl SuggestionModel {
    pub fn clear(&mut self) {
        self.items.clear();
        self.selected = None;
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn select_next(&mut self) {
        if self.items.is_empty() {
            self.selected = None;
            return;
        }
        self.selected = Some(match self.selected {
            None => 0,
            Some(i) => (i + 1) % self.items.len(),
        });
    }

    pub fn select_prev(&mut self) {
        if self.items.is_empty() {
            self.selected = None;
            return;
        }
        self.selected = Some(match self.selected {
            None => self.items.len() - 1,
            Some(0) => self.items.len() - 1,
            Some(i) => i - 1,
        });
    }

    pub fn selected_suggestion(&self) -> Option<&Suggestion> {
        self.selected.and_then(|i| self.items.get(i))
    }

    /// Wave B compatibility: open tabs + typed URL/search only.
    pub fn rebuild_local(
        &mut self,
        query: &str,
        open_tabs: &[(String, String)],
        provider: &SearchProvider,
    ) {
        let open: Vec<(String, String, usize)> = open_tabs
            .iter()
            .enumerate()
            .map(|(i, (t, u))| (t.clone(), u.clone(), i))
            .collect();
        self.rebuild_from_sources(query, &open, &[], &[], provider);
    }

    /// Merge history, bookmarks, open tabs, and typed URL/search — dedupe by URL.
    pub fn rebuild_from_sources(
        &mut self,
        query: &str,
        open_tabs: &[(String, String, usize)],
        history: &[HistoryRecord],
        bookmarks: &[Bookmark],
        provider: &SearchProvider,
    ) {
        self.items.clear();
        self.selected = None;
        let q = query.trim();
        if q.is_empty() {
            return;
        }
        let q_lower = q.to_ascii_lowercase();
        let mut by_url: std::collections::HashMap<String, Suggestion> =
            std::collections::HashMap::new();

        let push = |map: &mut std::collections::HashMap<String, Suggestion>, sug: Suggestion| {
            let key = sug.url.to_ascii_lowercase();
            match map.get_mut(&key) {
                Some(existing) => {
                    if sug.score > existing.score {
                        *existing = sug;
                    } else if sug.kind == SuggestionKind::Bookmark {
                        existing.kind = SuggestionKind::Bookmark;
                        existing.source = SuggestionSource::LocalBookmark;
                    } else if sug.open_tab_index.is_some() {
                        existing.open_tab_index = sug.open_tab_index;
                        existing.kind = SuggestionKind::OpenTab;
                        existing.title = sug.title;
                    }
                }
                None => {
                    map.insert(key, sug);
                }
            }
        };

        for (title, url, idx) in open_tabs {
            let t = title.to_ascii_lowercase();
            let u = url.to_ascii_lowercase();
            if t.contains(&q_lower) || u.contains(&q_lower) {
                push(
                    &mut by_url,
                    Suggestion {
                        kind: SuggestionKind::OpenTab,
                        source: SuggestionSource::OpenTabs,
                        title: format!("Switch to tab — {title}"),
                        url: url.clone(),
                        score: 95,
                        open_tab_index: Some(*idx),
                    },
                );
            }
        }

        let now = now_ms();
        for h in history {
            let age_h = ((now - h.visit_time_ms) as f64 / 3_600_000.0).max(0.0);
            let score = history_repo::ranking::score(
                true,
                age_h,
                h.visit_count,
                h.transition == VisitTransition::Typed,
            );
            push(
                &mut by_url,
                Suggestion {
                    kind: SuggestionKind::History,
                    source: SuggestionSource::LocalHistory,
                    title: h.title.clone(),
                    url: h.url.clone(),
                    score: score + 10,
                    open_tab_index: None,
                },
            );
        }

        for b in bookmarks {
            push(
                &mut by_url,
                Suggestion {
                    kind: SuggestionKind::Bookmark,
                    source: SuggestionSource::LocalBookmark,
                    title: format!("★ {}", b.title),
                    url: b.url.clone(),
                    score: 88,
                    open_tab_index: None,
                },
            );
        }

        match crate::classifier::OmniboxInputClassifier::classify(q) {
            crate::classifier::ClassifiedInput::Url(url) => {
                push(
                    &mut by_url,
                    Suggestion {
                        kind: SuggestionKind::Url,
                        source: SuggestionSource::TypedUrl,
                        title: url.clone(),
                        url,
                        score: 100,
                        open_tab_index: None,
                    },
                );
            }
            crate::classifier::ClassifiedInput::Search(terms) => {
                let url = provider.url_for_query(&terms);
                push(
                    &mut by_url,
                    Suggestion {
                        kind: SuggestionKind::Search,
                        source: SuggestionSource::SearchProvider,
                        title: format!("Search \"{terms}\""),
                        url,
                        score: 70,
                        open_tab_index: None,
                    },
                );
            }
        }

        self.items = by_url.into_values().collect();
        self.items.sort_by_key(|b| std::cmp::Reverse(b.score));
        if self.items.len() > 10 {
            self.items.truncate(10);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestion_keyboard_nav() {
        let mut m = SuggestionModel {
            items: vec![
                Suggestion {
                    kind: SuggestionKind::Url,
                    source: SuggestionSource::TypedUrl,
                    title: "a".into(),
                    url: "https://a.test/".into(),
                    score: 1,
                    open_tab_index: None,
                },
                Suggestion {
                    kind: SuggestionKind::Search,
                    source: SuggestionSource::SearchProvider,
                    title: "b".into(),
                    url: "https://b.test/".into(),
                    score: 1,
                    open_tab_index: None,
                },
            ],
            ..Default::default()
        };
        m.select_next();
        assert_eq!(m.selected, Some(0));
        m.select_next();
        assert_eq!(m.selected, Some(1));
    }

    #[test]
    fn dedupes_urls_across_sources() {
        let mut m = SuggestionModel::default();
        let hist = [HistoryRecord {
            id: "1".into(),
            url: "https://example.com/".into(),
            title: "Example".into(),
            visit_time_ms: now_ms(),
            visit_count: 3,
            transition: VisitTransition::Typed,
            host: "example.com".into(),
        }];
        let provider = SearchProvider::duckduckgo();
        m.rebuild_from_sources(
            "example",
            &[("Example".into(), "https://example.com/".into(), 0)],
            &hist,
            &[],
            &provider,
        );
        let count = m
            .items
            .iter()
            .filter(|s| s.url.contains("example.com"))
            .count();
        assert_eq!(count, 1);
    }
}
