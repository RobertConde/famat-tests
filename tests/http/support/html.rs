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

    /// The one GET form matching a CSS selector, ready to fill in.
    pub fn form(&self, css: &str) -> Form<'_> {
        let element = self.one(css);
        let method = element.value().attr("method").unwrap_or("get");
        assert!(method.eq_ignore_ascii_case("get"), "{css} is not a GET form");
        Form {
            element,
            selects: Vec::new(),
            checks: Vec::new(),
        }
    }
}

/// A GET form being filled in, submitted the way a browser with JavaScript
/// off does it: every named control in document order, unticked checkboxes
/// left out, encoded as `application/x-www-form-urlencoded`.
pub struct Form<'a> {
    element: ElementRef<'a>,
    selects: Vec<(String, String)>,
    checks: Vec<(String, String, bool)>,
}

impl Form<'_> {
    /// Pick the option whose value is `value` in the `name` dropdown.
    pub fn select(mut self, name: &str, value: &str) -> Self {
        self.selects.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Tick (or untick) the `name` checkbox whose value is `value`.
    pub fn check(mut self, name: &str, value: &str, ticked: bool) -> Self {
        self.checks.push((name.to_owned(), value.to_owned(), ticked));
        self
    }

    /// The path and query string the browser would GET.
    pub fn submit_url(&self) -> String {
        let action = self.element.value().attr("action").unwrap_or("");
        let mut url = reqwest::Url::parse("http://famat.test/").unwrap().join(action).unwrap();
        let controls = Selector::parse("select[name], input[name]").unwrap();
        let options = Selector::parse("option").unwrap();
        let mut pairs = Vec::new();
        for control in self.element.select(&controls) {
            let name = control.value().attr("name").unwrap();
            if control.value().name() == "select" {
                let options: Vec<_> = control.select(&options).collect();
                let value_of = |o: &ElementRef| o.value().attr("value").map_or_else(|| text(*o), str::to_owned);
                let chosen = match self.selects.iter().find(|(n, _)| n == name) {
                    Some((_, value)) => options
                        .iter()
                        .find(|o| value_of(o) == *value)
                        .unwrap_or_else(|| panic!("the {name} dropdown has no option {value:?}")),
                    None => options
                        .iter()
                        .find(|o| o.value().attr("selected").is_some())
                        .or(options.first())
                        .unwrap_or_else(|| panic!("the {name} dropdown has no options")),
                };
                pairs.push((name.to_owned(), value_of(chosen)));
                continue;
            }
            let value = control.value().attr("value").unwrap_or("");
            match control.value().attr("type").unwrap_or("text") {
                "checkbox" | "radio" => {
                    let ticked = self
                        .checks
                        .iter()
                        .find(|(n, v, _)| n == name && v == value)
                        .map_or(control.value().attr("checked").is_some(), |(_, _, ticked)| *ticked);
                    if ticked {
                        pairs.push((name.to_owned(), value.to_owned()));
                    }
                }
                "submit" | "button" | "reset" | "image" | "file" => {}
                _ => pairs.push((name.to_owned(), value.to_owned())),
            }
        }
        for (name, value, _) in &self.checks {
            let css = format!("input[name={name:?}][value={value:?}]");
            let selector = Selector::parse(&css).unwrap();
            assert!(
                self.element.select(&selector).next().is_some(),
                "the form has no {name}={value} checkbox"
            );
        }
        url.query_pairs_mut().clear().extend_pairs(&pairs);
        match url.query() {
            Some(query) if !query.is_empty() => format!("{}?{query}", url.path()),
            _ => url.path().to_owned(),
        }
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
