//! A web page as plain text for the model: only the content, no head, scripts, navigation or forms.
//! Headings become `## `, list items `- `, everything else lines of text. Every character costs tokens.

use std::collections::HashSet;

/// Upper limit for the text of one page.
pub const PAGE_CHARS: usize = 4000;
/// An article this long is the page's content; shorter ones are teasers of an overview page.
const ARTICLE_CHARS: usize = 600;
const TEASER_CHARS: usize = 240;
const TEASERS: usize = 8;

/// Elements dropped with everything inside them.
const DROP: &[&str] = &[
    "script", "style", "noscript", "svg", "nav", "aside", "footer", "form", "iframe", "template",
    "button", "select", "dialog", "figure", "picture", "video", "audio", "canvas", "label",
];

/// Elements without content or closing tag; `hidden` on them has nothing to drop.
const VOID: &[&str] = &[
    "img", "input", "br", "hr", "meta", "link", "source", "wbr", "area", "col", "embed", "track",
];

/// Marks link text inside `html_text` until `tidy` has judged the line.
const LINK_START: char = '\u{1}';
const LINK_END: char = '\u{2}';
/// After `</span>`: a word that follows directly gets a space ("Digitalgipfel" + "Merz: …").
const SPAN_END: char = '\u{3}';

pub struct Readable<'a> {
    pub text: String,
    /// The part of the HTML the text came from, for picking further links.
    pub region: &'a str,
}

/// One long article, otherwise the teasers of an overview page, otherwise `<main>` or the body.
pub fn readable(html: &str) -> Readable<'_> {
    let body = body_of(html);
    let articles = elements(body, "article");
    let longest = articles
        .iter()
        .map(|article| (html_text(article, true), *article))
        .max_by_key(|(text, _)| text.chars().count());
    if let Some((text, article)) = longest {
        if text.chars().count() >= ARTICLE_CHARS {
            return Readable {
                text: crate::authz::truncate(&text, PAGE_CHARS),
                region: article,
            };
        }
    }
    if articles.len() >= 2 {
        let teasers = articles
            .iter()
            .map(|article| {
                html_text(article, true)
                    .replace("## ", "")
                    .replace('\n', " ")
            })
            .filter(|teaser| teaser.chars().count() > 40)
            .take(TEASERS)
            .map(|teaser| format!("- {}", crate::authz::truncate(&teaser, TEASER_CHARS)))
            .collect::<Vec<_>>();
        if !teasers.is_empty() {
            return Readable {
                text: crate::authz::truncate(&teasers.join("\n"), PAGE_CHARS),
                region: elements(body, "main").first().copied().unwrap_or(body),
            };
        }
    }
    let (region, keep_header) = match elements(body, "main").first() {
        Some(main) => (*main, true),
        None => (body, false),
    };
    Readable {
        text: crate::authz::truncate(&html_text(region, keep_header), PAGE_CHARS),
        region,
    }
}

/// Everything after `<head>`. Pages without a body tag are taken whole.
fn body_of(html: &str) -> &str {
    let lower = html.to_ascii_lowercase();
    let start = match find_tag(&lower, 0, "body") {
        Some(at) => lower[at..].find('>').map(|end| at + end + 1).unwrap_or(at),
        None => lower.find("</head>").map(|at| at + 7).unwrap_or(0),
    };
    let end = lower
        .rfind("</body")
        .filter(|end| *end >= start)
        .unwrap_or(html.len());
    &html[start..end]
}

/// Inner HTML of every `<tag>` that is not inside another one of the same name.
fn elements<'a>(html: &'a str, tag: &str) -> Vec<&'a str> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(start) = find_tag(&lower, from, tag) {
        let Some(open) = lower[start..].find('>') else {
            break;
        };
        let inner = start + open + 1;
        let end = element_end(&lower, inner, tag).unwrap_or(lower.len());
        let close = lower[inner..end]
            .rfind("</")
            .map(|at| inner + at)
            .unwrap_or(end);
        out.push(&html[inner..close]);
        from = end;
    }
    out
}

/// Position of the next `<tag` that is exactly this tag (`<main` but not `<mainmenu`).
fn find_tag(lower: &str, from: usize, tag: &str) -> Option<usize> {
    find_exact(lower, from, &format!("<{tag}"))
}

/// Position of the next `</tag`, again not a longer name.
fn find_close(lower: &str, from: usize, tag: &str) -> Option<usize> {
    find_exact(lower, from, &format!("</{tag}"))
}

fn find_exact(lower: &str, from: usize, needle: &str) -> Option<usize> {
    let mut at = from;
    while let Some(rel) = lower.get(at..)?.find(needle) {
        let start = at + rel;
        let next = lower.as_bytes().get(start + needle.len()).copied();
        if matches!(
            next,
            Some(b' ' | b'>' | b'/' | b'\t' | b'\n' | b'\r') | None
        ) {
            return Some(start);
        }
        at = start + needle.len();
    }
    None
}

/// End (after `>`) of the element whose content starts at `from`, same-named children counted.
/// `None` when it is never closed.
fn element_end(lower: &str, from: usize, tag: &str) -> Option<usize> {
    let mut depth = 1usize;
    let mut at = from;
    loop {
        let next_open = find_tag(lower, at, tag);
        let next_close = find_close(lower, at, tag)?;
        match next_open {
            Some(open) if open < next_close => {
                depth += 1;
                at = open + 1;
            }
            _ => {
                depth -= 1;
                at = lower[next_close..]
                    .find('>')
                    .map(|end| next_close + end + 1)
                    .unwrap_or(lower.len());
                if depth == 0 {
                    return Some(at);
                }
            }
        }
    }
}

/// HTML to lines of text. `keep_header` keeps `<header>`, which inside an article holds the title.
fn html_text(html: &str, keep_header: bool) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::new();
    let mut pre = 0usize;
    let mut i = 0;
    while i < html.len() {
        let Some(rel) = html[i..].find('<') else {
            push_text(&mut out, &html[i..], pre > 0);
            break;
        };
        push_text(&mut out, &html[i..i + rel], pre > 0);
        let start = i + rel;
        if lower[start..].starts_with("<!--") {
            i = lower[start..]
                .find("-->")
                .map(|end| start + end + 3)
                .unwrap_or(html.len());
            continue;
        }
        let Some(close) = html[start..].find('>') else {
            break;
        };
        let end = start + close + 1;
        let tag = &lower[start + 1..end - 1];
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        i = end;
        let dropped = DROP.contains(&name.as_str())
            || (!keep_header && name == "header")
            || (hidden(tag) && !VOID.contains(&name.as_str()));
        if dropped && !closing && !tag.ends_with('/') {
            // An element that is never closed loses only its tag, not the rest of the page.
            if let Some(after) = element_end(&lower, end, &name) {
                i = after;
            }
            continue;
        }
        if (name == "ul" || name == "ol") && !closing {
            if let Some(list_end) = element_end(&lower, end, &name) {
                if link_list(&lower[end..list_end]) {
                    i = list_end;
                    continue;
                }
            }
        }
        match name.as_str() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                out.push_str(if closing { "\n" } else { "\n## " });
            }
            "li" if !closing => out.push_str("\n- "),
            "a" if !closing => {
                let before = out.trim_end_matches(SPAN_END);
                if before.ends_with(|c: char| c.is_alphanumeric()) {
                    out.push(' ');
                }
                out.push(LINK_START);
            }
            "a" => out.push(LINK_END),
            "span" if closing => out.push(SPAN_END),
            "pre" => {
                pre = if closing {
                    pre.saturating_sub(1)
                } else {
                    pre + 1
                };
                out.push('\n');
            }
            "td" | "th" if !closing => {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push_str(" | ");
                }
            }
            "br" | "p" | "div" | "section" | "article" | "main" | "header" | "ul" | "ol"
            | "table" | "tr" | "blockquote" | "dl" | "dt" | "dd" | "figcaption" | "hr" => {
                out.push('\n')
            }
            _ => {}
        }
    }
    tidy(&out)
}

/// Not shown on the page: `hidden`, `aria-hidden="true"` or `display:none` on the tag.
fn hidden(tag: &str) -> bool {
    let compact: String = tag.chars().filter(|c| !c.is_whitespace()).collect();
    // Material icon fonts write the icon's name ("calendar_month") as text.
    if compact.contains("aria-hidden=\"true\"")
        || compact.contains("display:none")
        || compact.contains("material-icons")
        || compact.contains("material-symbols")
    {
        return true;
    }
    // The bare attribute, not the word inside a value like class="hidden md:block".
    let mut quote = None;
    let mut word = String::new();
    for c in tag.chars().chain([' ']) {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c.is_whitespace() || c == '=' || c == '/' => {
                if word == "hidden" {
                    return true;
                }
                word.clear();
            }
            None => word.push(c),
        }
    }
    false
}

/// A list of two or more items that are nothing but links: contents, menus, share and
/// "read also" boxes. The pages behind them are offered as further links anyway.
fn link_list(lower: &str) -> bool {
    let items = lower.matches("<li").count();
    if items < 2 {
        return false;
    }
    let mut rest = String::new();
    let mut at = 0;
    while let Some(start) = find_tag(lower, at, "a") {
        rest.push_str(&lower[at..start]);
        at = find_close(lower, start, "a")
            .and_then(|close| lower[close..].find('>').map(|end| close + end + 1))
            .unwrap_or(lower.len());
    }
    rest.push_str(&lower[at..]);
    let text = html_text(&rest, true);
    text.chars().filter(|c| c.is_alphanumeric()).count() < items * 3
}

/// Text between tags with entities decoded and any run of whitespace as one space.
/// Inside `<pre>` line breaks stay, so code keeps its lines.
fn push_text(out: &mut String, raw: &str, pre: bool) {
    for c in decode_entities(raw).chars() {
        // Soft hyphens, zero-width characters and icon glyphs from the private use area.
        if matches!(
            c,
            '\u{ad}' | '\u{200b}' | '\u{feff}' | '\u{e000}'..='\u{f8ff}'
        ) {
            continue;
        }
        let after_span = out.ends_with(SPAN_END);
        if after_span {
            out.pop();
        }
        if pre && c == '\n' {
            out.push('\n');
        } else if c.is_whitespace() {
            if !out.is_empty() && !out.ends_with(' ') && !out.ends_with('\n') {
                out.push(' ');
            }
        } else {
            let word_before = out.ends_with(|before: char| before.is_alphanumeric());
            if c.is_alphanumeric() && (out.ends_with(LINK_END) || (after_span && word_before)) {
                out.push(' ');
            }
            out.push(c);
        }
    }
}

/// Trimmed lines without empty headings and bullets, symbols, embedded JSON, repeats (share
/// buttons, cookie notes), menus of links and short lines that are only a link ("Bei Amazon
/// ansehen", "Links bearbeiten"). Reference marks like `[2]` go too.
fn tidy(text: &str) -> String {
    let mut seen = HashSet::new();
    let mut lines = Vec::new();
    for raw in text.lines() {
        let line = without_citations(&raw.replace([LINK_START, LINK_END, SPAN_END], ""))
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if line.is_empty() || !line.chars().any(char::is_alphanumeric) {
            continue;
        }
        if (line.starts_with('{') || line.starts_with('[')) && line.contains("\":") {
            continue;
        }
        let plain = !line.starts_with("- ") && !line.starts_with("## ");
        let links = raw.matches(LINK_START).count();
        if only_link(raw) && (links >= 3 || (plain && line.split(' ').count() <= 4)) {
            continue;
        }
        // Edit and jump marks such as "[Bearbeiten | Quelltext bearbeiten]".
        if line.starts_with('[') && line.ends_with(']') && line.chars().count() < 60 {
            continue;
        }
        if seen.insert(line.clone()) {
            lines.push(line);
        }
    }
    // A heading followed by another heading or by nothing has no text of its own: menus.
    let heading = |line: &String| line.starts_with("## ");
    lines
        .iter()
        .enumerate()
        .filter(|(i, line)| !heading(line) || lines.get(i + 1).is_some_and(|next| !heading(next)))
        .map(|(_, line)| line.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Removes reference marks such as `[2]` or `[11]`.
fn without_citations(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find('[') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let digits = after.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && after[digits..].starts_with(']') {
            rest = &after[digits + 1..];
        } else {
            out.push('[');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Every visible character of the line sits between link marks.
fn only_link(line: &str) -> bool {
    let mut inside = false;
    let mut any = false;
    for c in line.chars() {
        match c {
            LINK_START => inside = true,
            LINK_END => inside = false,
            SPAN_END => {}
            c if c.is_whitespace() => {}
            _ if inside => any = true,
            _ => return false,
        }
    }
    any
}

/// Links in a piece of HTML as (address as written, visible text), in page order.
pub fn links(html: &str) -> Vec<(String, String)> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(start) = find_tag(&lower, from, "a") {
        let Some(open) = lower[start..].find('>') else {
            break;
        };
        let inner = start + open + 1;
        let close = find_close(&lower, inner, "a").unwrap_or(lower.len());
        from = close.max(inner);
        let Some(href) = attribute(&html[start..inner], "href") else {
            continue;
        };
        let text = html_text(&html[inner..close], true)
            .replace("## ", "")
            .replace('\n', " ");
        out.push((decode_entities(href.trim()), text));
    }
    out
}

/// Value of an attribute in an opening tag, quoted or not.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    loop {
        let at = from + lower[from..].find(name)?;
        from = at + name.len();
        let before = lower[..at].chars().last();
        if !matches!(before, Some(c) if c.is_whitespace()) {
            continue;
        }
        let rest = lower[from..].trim_start();
        let Some(value) = rest.strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        let offset = tag.len() - value.len();
        return match value.chars().next()? {
            quote @ ('"' | '\'') => {
                let end = value[1..].find(quote)?;
                Some(&tag[offset + 1..offset + 1 + end])
            }
            _ => {
                let end = value
                    .find(|c: char| c.is_whitespace() || c == '>')
                    .unwrap_or(value.len());
                Some(&tag[offset..offset + end])
            }
        };
    }
}

pub fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest[1..]
            .find(';')
            .filter(|end| *end <= 10)
            .and_then(|end| entity(&rest[1..end + 1]).map(|c| (c, end + 2)));
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(code);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "shy" => '\u{ad}',
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "bdquo" => '„',
        "ldquo" => '“',
        "rdquo" => '”',
        "sbquo" => '‚',
        "lsquo" => '‘',
        "rsquo" => '’',
        "laquo" => '«',
        "raquo" => '»',
        "auml" => 'ä',
        "ouml" => 'ö',
        "uuml" => 'ü',
        "Auml" => 'Ä',
        "Ouml" => 'Ö',
        "Uuml" => 'Ü',
        "szlig" => 'ß',
        "eacute" => 'é',
        "euro" => '€',
        "copy" => '©',
        "reg" => '®',
        "middot" => '·',
        "bull" => '•',
        "deg" => '°',
        "times" => '×',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_and_page_chrome_stay_out() {
        let html = format!(
            "<html><head><title>T</title><style>{}</style></head><body>\
             <header><a href=\"/\">Logo</a></header><nav>Menü</nav>\
             <main><h1>Titel</h1><p>Erster&nbsp;Absatz mit &bdquo;Zitat&ldquo;.</p>\
             <script>var token = 1;</script><ul><li>eins</li><li>zwei</li></ul>\
             <form><button>Absenden</button></form></main><footer>Impressum</footer></body></html>",
            "x".repeat(200_000)
        );
        let page = readable(&html);
        assert_eq!(
            page.text,
            "## Titel\nErster Absatz mit „Zitat“.\n- eins\n- zwei"
        );
    }

    #[test]
    fn one_long_article_is_read_whole_with_its_title() {
        let body = "Der Pi 5 braucht im Leerlauf etwa drei Watt. ".repeat(20);
        let html = format!(
            "<body><main><article><header><h1>Stromverbrauch</h1></header><p>{body}</p>\
             <aside>Werbung</aside></article>\
             <article><p>Verwandter Artikel mit kurzem Anriss, der nicht gelesen wird.</p></article></main></body>"
        );
        let page = readable(&html);
        assert!(page.text.starts_with("## Stromverbrauch\nDer Pi 5"));
        assert!(!page.text.contains("Werbung"));
        assert!(!page.text.contains("Verwandter"));
        assert!(page.region.contains("Stromverbrauch"));
    }

    #[test]
    fn overview_pages_become_teasers() {
        let html = "<nav>Menü</nav><article><h2>Haushaltsstreit in Belgien</h2><p>De Wever geht zum König.</p></article>\
                    <article><p>Kurzer Krümel</p></article>\
                    <article><h2>Wahl in Bayern</h2><p>Die Auszählung läuft noch bis morgen früh.</p></article>";
        let page = readable(html);
        assert_eq!(
            page.text,
            "- Haushaltsstreit in Belgien De Wever geht zum König.\n- Wahl in Bayern Die Auszählung läuft noch bis morgen früh."
        );
    }

    #[test]
    fn repeated_lines_and_comments_are_dropped() {
        let page =
            readable("<body><p>Teilen</p><!-- <p>alt</p> --><p>Inhalt</p><p>Teilen</p></body>");
        assert_eq!(page.text, "Teilen\nInhalt");
    }

    #[test]
    fn nested_dropped_elements_are_skipped_whole() {
        let page = readable(
            "<body><div><nav><ul><li><nav>x</nav></li></ul>Rest vom Menü</nav>Text</div></body>",
        );
        assert_eq!(page.text, "Text");
    }

    #[test]
    fn link_menus_go_and_real_lists_stay() {
        let toc = "<li><a href=\"#a\">Überblick</a></li>".repeat(6);
        let langs =
            "<li><a href=\"https://en.wikipedia.org/\"><span>English</span></a></li>".repeat(8);
        let short =
            "<li><a href=\"/a/b\">NVMe-Boot</a></li><li><a href=\"/c/d\">Pi Zero sparen</a></li>";
        let facts = "<li>Pi 4: 2,9 W</li><li>Pi 5: 3,0 W</li><li>Zero 2 W: 0,6 W</li><li>Zero W: 0,3 W</li><li>3 B+: 1,9 W</li>";
        let html = format!(
            "<main><ul>{toc}</ul><ul>{langs}</ul><h2>Werte</h2><ul>{facts}</ul>\
             <h2>Weiterlesen</h2><ul>{short}</ul><p>[Bearbeiten | Quelltext bearbeiten]</p></main>"
        );
        let text = readable(&html).text;
        assert_eq!(
            text,
            "## Werte\n- Pi 4: 2,9 W\n- Pi 5: 3,0 W\n- Zero 2 W: 0,6 W\n- Zero W: 0,3 W\n- 3 B+: 1,9 W"
        );
    }

    #[test]
    fn hidden_elements_stay_out() {
        let page = readable(
            "<body><span class=\"material-icons\" aria-hidden=\"true\">calendar_month</span>\
             <div hidden><p>Menü</p></div><div style=\"display: none\">Modal</div>\
             <div class=\"hidden md:block\">sichtbar auf dem Desktop</div>\
             <img hidden src=\"x.png\"><p>Erstellt am 21.4.2007</p></body>",
        );
        assert_eq!(page.text, "sichtbar auf dem Desktop\nErstellt am 21.4.2007");
    }

    #[test]
    fn menus_symbols_json_and_marks_go() {
        let menu = ["HTML", "CSS", "JAVASCRIPT", "SQL", "PYTHON"]
            .map(|item| format!("<a href=\"/{item}\">{item}</a> "))
            .concat();
        let html = format!(
            "<body><div>{menu}</div><div>×</div><div>\u{e970}</div>\
             <div>{{\"displayPrice\":\"16,37 €\"}}</div>\
             <p>Er ist ein Einplatinencomputer.[2] Bis 2022 über 45 Millionen.[11][12]</p>\
             <p>Liste [a] bleibt</p></body>"
        );
        let page = readable(&html);
        assert_eq!(
            page.text,
            "Er ist ein Einplatinencomputer. Bis 2022 über 45 Millionen.\nListe [a] bleibt"
        );
    }

    #[test]
    fn broken_html_does_not_panic() {
        for html in [
            "",
            "<",
            "<<a",
            "</>",
            "<!--",
            "<>text",
            "ä<ö>ü</ö>ß",
            "&",
            "&#xFFFFFFF;&#55296;&;",
            "<div title='a>b'>Text</div>",
            "<body><main><article>",
            "<ul><li><a href=x>",
            "<pre>\n\n</pre>",
            "<span aria-hidden=\"true\">",
            "Grüße [ 1 ] [9",
        ] {
            let _ = readable(html);
        }
        assert_eq!(readable("ä<ö>ü</ö>ß").text, "äüß");
    }

    #[test]
    fn links_come_with_their_text() {
        let html = "<p>Siehe <a class=\"x\" href=\"/a?x=1&amp;y=2\">Teil <b>eins</b></a>, \
                    <A HREF='https://b.de/c'>zwei</A>, <a href=/ohne-anfuehrung>drei</a>, \
                    <a name=\"anker\">kein Link</a>, <a data-href=\"/falsch\" href=\"/richtig\">vier</a> \
                    <a href=\"/kaputt";
        assert_eq!(
            links(html),
            vec![
                ("/a?x=1&y=2".to_string(), "Teil eins".to_string()),
                ("https://b.de/c".to_string(), "zwei".to_string()),
                ("/ohne-anfuehrung".to_string(), "drei".to_string()),
                ("/richtig".to_string(), "vier".to_string()),
            ]
        );
    }

    #[test]
    fn headings_without_text_go() {
        let page = readable(
            "<body><h2>Python Classes</h2><h2>File Handling</h2><h1>Hintergrund</h1><h2>Idee</h2>\
             <p>Der Text.</p><span class=\"material-symbols-outlined\">schedule</span><h2>Kommentare</h2></body>",
        );
        assert_eq!(page.text, "## Idee\nDer Text.");
    }

    #[test]
    fn code_keeps_its_lines() {
        let page = readable(
            "<body><p>Beispiel:</p><pre><code>a = [2, 3]\nres = [v ** 2 for v in a]\nprint(res)</code></pre></body>",
        );
        assert_eq!(
            page.text,
            "Beispiel:\na = [2, 3]\nres = [v ** 2 for v in a]\nprint(res)"
        );
    }

    #[test]
    fn link_text_does_not_stick_to_words() {
        let page = readable(
            "<body><p>Von<a href=\"/autor\">Philipp Schweizer</a>am 30.10.</p>\
             <p><a href=\"/x\">Mehr</a>.</p></body>",
        );
        assert_eq!(page.text, "Von Philipp Schweizer am 30.10.\nMehr.");
        let teaser = readable(
            "<body><h3><span>Digitalgipfel</span><span>Merz: Cloud</span></h3><p>Lese<span>dauer</span>: \
             <span>2</span><span>,5</span> Min.</p></body>",
        );
        assert_eq!(
            teaser.text,
            "## Digitalgipfel Merz: Cloud\nLesedauer: 2,5 Min."
        );
        let mixed = readable("<body><p><span>Quelle</span><a href=\"/x\">dpa</a></p></body>");
        assert_eq!(mixed.text, "Quelle dpa");
    }

    #[test]
    fn short_lines_that_are_only_a_link_go() {
        let page = readable(
            "<body><p>Preis ca. 25 €</p><div><a href=\"https://amazon.de/x\">Bei Amazon ansehen</a></div>\
             <span><a href=\"#\">Links bearbeiten</a></span><label>65 Sprachen</label>\
             <h2><a href=\"/a/b\">Überschrift als Link</a></h2>\
             <p><a href=\"/a/c\">Ein langer verlinkter Satz mit mehr als vier Wörtern</a></p>\
             <ul><li><a href=\"/a/d\">Weiterlesen</a></li></ul></body>",
        );
        assert_eq!(
            page.text,
            "Preis ca. 25 €\n## Überschrift als Link\nEin langer verlinkter Satz mit mehr als vier Wörtern\n- Weiterlesen"
        );
    }

    #[test]
    fn table_cells_are_separated() {
        let page = readable(
            "<body><table><tr><th>Modell</th><th>Leerlauf</th></tr>\
             <tr><td>Pi 5</td><td>3,0 W</td></tr></table></body>",
        );
        assert_eq!(page.text, "Modell | Leerlauf\nPi 5 | 3,0 W");
    }

    #[test]
    fn an_unclosed_element_does_not_swallow_the_page() {
        let page = readable("<body><button>Menü<p>Der eigentliche Text.</p></body>");
        assert_eq!(page.text, "Menü\nDer eigentliche Text.");
        assert_eq!(readable("<body><ul><li>offen").text, "- offen");
    }

    #[test]
    fn similar_tag_names_are_not_confused() {
        assert_eq!(
            elements("<mainmenu>a</mainmenu><main>b</main>", "main"),
            vec!["b"]
        );
        assert_eq!(readable("<body><navi>bleibt</navi></body>").text, "bleibt");
    }

    #[test]
    fn entities_are_decoded() {
        assert_eq!(
            decode_entities(
                "Gr&ouml;&szlig;e &amp; Preis: 5&#8239;&euro; &#x2013; &unknown; & mehr"
            ),
            "Größe & Preis: 5\u{202f}€ – &unknown; & mehr"
        );
    }
}
