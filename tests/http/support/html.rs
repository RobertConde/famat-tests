//! Reading server-rendered HTML the way a browser with JavaScript off sees
//! it. Content inside `<template>` or only added by scripts is not visible.

use scraper::{ElementRef, Html, Selector};

pub struct Page {
    html: Html,
}

impl Page {
    pub fn parse(source: &str) -> Self {
        Self {
            html: Html::parse_document(source),
        }
    }

    /// Every element matching a CSS selector.
    pub fn select(&self, css: &str) -> Vec<ElementRef<'_>> {
        let selector = Selector::parse(css).unwrap_or_else(|e| panic!("bad selector {css:?}: {e}"));
        self.html.select(&selector).collect()
    }

    /// The one element matching a CSS selector.
    pub fn one(&self, css: &str) -> ElementRef<'_> {
        let found = self.select(css);
        assert_eq!(found.len(), 1, "expected exactly one {css:?}, found {}", found.len());
        found[0]
    }

    /// The visible text of the one element matching a CSS selector.
    pub fn text(&self, css: &str) -> String {
        text(self.one(css))
    }
}

/// An element's text with whitespace collapsed.
pub fn text(element: ElementRef<'_>) -> String {
    element
        .text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The text of the one descendant matching a CSS selector.
pub fn text_in(element: ElementRef<'_>, css: &str) -> String {
    let selector = Selector::parse(css).unwrap();
    let found: Vec<_> = element.select(&selector).collect();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one {css:?} in the element, found {}",
        found.len()
    );
    text(found[0])
}

/// `(text, href)` of every link under an element.
pub fn links_in(element: ElementRef<'_>) -> Vec<(String, String)> {
    let selector = Selector::parse("a[href]").unwrap();
    element
        .select(&selector)
        .map(|a| (text(a), a.value().attr("href").unwrap().to_owned()))
        .collect()
}
