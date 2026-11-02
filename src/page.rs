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
    "button", "select", "dialog", "figure", "picture", "video", "audio", "canvas",
];

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
        let end = element_end(&lower, inner, tag);
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
fn element_end(lower: &str, from: usize, tag: &str) -> usize {
    let mut depth = 1usize;
    let mut at = from;
    loop {
        let next_open = find_tag(lower, at, tag);
        let Some(next_close) = find_close(lower, at, tag) else {
            return lower.len();
        };
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
                    return at;
                }
            }
        }
    }
}

/// HTML to lines of text. `keep_header` keeps `<header>`, which inside an article holds the title.
fn html_text(html: &str, keep_header: bool) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::new();
    let mut i = 0;
    while i < html.len() {
        let Some(rel) = html[i..].find('<') else {
            push_text(&mut out, &html[i..]);
            break;
        };
        push_text(&mut out, &html[i..i + rel]);
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
        let dropped = DROP.contains(&name.as_str()) || (!keep_header && name == "header");
        if dropped && !closing && !tag.ends_with('/') {
            i = element_end(&lower, end, &name);
            continue;
        }
        if (name == "ul" || name == "ol") && !closing {
            let list_end = element_end(&lower, end, &name);
            if link_list(&lower[end..list_end]) {
                i = list_end;
                continue;
            }
        }
        match name.as_str() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                out.push_str(if closing { "\n" } else { "\n## " });
            }
            "li" if !closing => out.push_str("\n- "),
            "td" | "th" if !closing => out.push(' '),
            "br" | "p" | "div" | "section" | "article" | "main" | "header" | "ul" | "ol"
            | "table" | "tr" | "blockquote" | "pre" | "dl" | "dt" | "dd" | "figcaption" | "hr" => {
                out.push('\n')
            }
            _ => {}
        }
    }
    tidy(&out)
}

/// A list of five or more items that are nothing but links: contents, language or tag menus.
fn link_list(lower: &str) -> bool {
    let items = lower.matches("<li").count();
    if items < 5 {
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
fn push_text(out: &mut String, raw: &str) {
    for c in decode_entities(raw).chars() {
        if c == '\u{ad}' || c == '\u{200b}' || c == '\u{feff}' {
            continue;
        }
        if c.is_whitespace() {
            if !out.is_empty() && !out.ends_with(' ') && !out.ends_with('\n') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
}

/// Trimmed lines without empty headings, empty bullets and repeats (share buttons, cookie notes).
fn tidy(text: &str) -> String {
    let mut seen = HashSet::new();
    let mut lines = Vec::new();
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() || line == "-" || line == "##" {
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
    lines.join("\n")
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
            "## Werte\n- Pi 4: 2,9 W\n- Pi 5: 3,0 W\n- Zero 2 W: 0,6 W\n- Zero W: 0,3 W\n- 3 B+: 1,9 W\n## Weiterlesen\n- NVMe-Boot\n- Pi Zero sparen"
        );
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
