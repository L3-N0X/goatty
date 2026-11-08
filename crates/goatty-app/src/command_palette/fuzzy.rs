//! Fuzzy matching for the command palette, in the style of fzf/VS Code.
//!
//! A query is split on whitespace into words; every word has to match the
//! label as a case-insensitive subsequence, in any order ("settings open"
//! finds "Settings: Open settings"). Each word is aligned with a small
//! dynamic program (O(query × label)) that prefers, in order of weight:
//! matches at word starts (`Open Settings` for `os`), runs of consecutive
//! characters, few and short gaps, and matches early in the label.
//!
//! Scoring everything on every keystroke is cheap enough: 500 labels of
//! 75 characters against a two-word query take ~1.0 ms in a release build
//! (~1.3 ms in dev; `scoring_is_fast` prints it). Real labels are about a
//! third as long and there are far fewer of them.

/// A successful match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Higher is better. Only comparable between matches of the same query.
    pub score: i32,
    /// Matched character (not byte) indices in the label, sorted, unique.
    pub positions: Vec<usize>,
}

// Weights follow fzf's (scoreMatch 16, boundary bonuses ~8-10, gaps -3/-1).
const MATCH: i32 = 16;
/// The first character of the label, or one after whitespace.
const BONUS_WORD: i32 = 10;
/// After a delimiter (`:`, `-`, `_`, `/`, `.`, …), a lower→upper case change, or a letter→digit change.
const BONUS_BOUNDARY: i32 = 8;
/// Least bonus for a character matched right after the previous one. A run
/// otherwise inherits the bonus of its first character, so a whole word
/// matched from its start scores like a string of word starts.
const BONUS_CONSECUTIVE: i32 = 4;
/// The first query character's bonus counts this many times.
const FIRST_CHAR_MULTIPLIER: i32 = 2;
const GAP_START: i32 = 3;
const GAP_EXTEND: i32 = 1;
/// Per skipped character before the first match, capped (categories make
/// labels start with a prefix, so this is only a tie-breaker).
const LEADING: i32 = 1;
const LEADING_MAX: i32 = 3;
/// The label starts with the query word.
const BONUS_PREFIX: i32 = 8;

/// Match `query` against `label`. An empty or all-whitespace query matches
/// everything with score 0.
pub fn fuzzy_match(query: &str, label: &str) -> Option<Match> {
    let hay: Vec<char> = label.chars().collect();
    let lower: Vec<char> = hay.iter().map(|c| fold(*c)).collect();
    let bonus: Vec<i32> = (0..hay.len()).map(|i| boundary_bonus(&hay, i)).collect();
    let mut score = 0;
    let mut positions = Vec::new();
    for word in query.split_whitespace() {
        let needle: Vec<char> = word.chars().map(fold).collect();
        let m = align(&needle, &lower, &bonus)?;
        score += m.score;
        positions.extend(m.positions);
    }
    positions.sort_unstable();
    positions.dedup();
    Some(Match { score, positions })
}

/// Case folding for matching (one char to one char, so indices line up).
fn fold(c: char) -> char {
    if c.is_ascii() { c.to_ascii_lowercase() } else { c.to_lowercase().next().unwrap_or(c) }
}

fn is_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, ':' | '-' | '_' | '/' | '.' | ',' | '(' | ')' | '&' | '+')
}

fn boundary_bonus(hay: &[char], i: usize) -> i32 {
    let cur = hay[i];
    if is_separator(cur) {
        return 0;
    }
    let Some(&prev) = i.checked_sub(1).map(|p| &hay[p]) else { return BONUS_WORD };
    if prev.is_whitespace() {
        BONUS_WORD
    } else if is_separator(prev) || (prev.is_lowercase() && cur.is_uppercase()) || (!prev.is_numeric() && cur.is_numeric()) {
        BONUS_BOUNDARY
    } else {
        0
    }
}

/// Best alignment of one query word (already folded) in the folded label.
fn align(needle: &[char], hay: &[char], bonus: &[i32]) -> Option<Match> {
    let (n, m) = (needle.len(), hay.len());
    if n == 0 {
        return Some(Match { score: 0, positions: Vec::new() });
    }
    if n > m || !is_subsequence(needle, hay) {
        return None;
    }
    const NONE: i32 = i32::MIN / 4;
    // d[i*m + j]: best score with needle[..=i] matched and needle[i] at hay[j].
    // from[i*m + j]: where needle[i-1] was matched in that best alignment.
    // run[i*m + j]: the bonus the current consecutive run inherits.
    let mut d = vec![NONE; n * m];
    let mut from = vec![usize::MAX; n * m];
    let mut run = vec![0; n * m];
    for j in 0..m {
        if hay[j] == needle[0] {
            d[j] = MATCH + bonus[j] * FIRST_CHAR_MULTIPLIER - (j as i32 * LEADING).min(LEADING_MAX);
            run[j] = bonus[j];
        }
    }
    for (i, &want) in needle.iter().enumerate().skip(1) {
        let (p, cur) = ((i - 1) * m, i * m);
        // best: max over k <= j-2 of d[p+k] - gap(j-1-k), with its k. Skipping
        // g characters costs GAP_START + (g-1)*GAP_EXTEND.
        let mut best = NONE;
        let mut best_at = usize::MAX;
        for j in 1..m {
            if j >= 2 {
                if best > NONE {
                    best -= GAP_EXTEND;
                }
                let cand = d[p + j - 2] - GAP_START;
                if cand > best {
                    best = cand;
                    best_at = j - 2;
                }
            }
            if hay[j] != want {
                continue;
            }
            let run_bonus = run[p + j - 1].max(bonus[j]).max(BONUS_CONSECUTIVE);
            let consecutive = if d[p + j - 1] > NONE { d[p + j - 1] + run_bonus } else { NONE };
            let gapped = if best > NONE { best + bonus[j] } else { NONE };
            if consecutive == NONE && gapped == NONE {
                continue;
            }
            if consecutive >= gapped {
                d[cur + j] = consecutive + MATCH;
                from[cur + j] = j - 1;
                run[cur + j] = run_bonus;
            } else {
                d[cur + j] = gapped + MATCH;
                from[cur + j] = best_at;
                run[cur + j] = bonus[j];
            }
        }
    }
    let last = (n - 1) * m;
    let (end, &score) = d[last..].iter().enumerate().max_by_key(|&(j, s)| (*s, std::cmp::Reverse(j)))?;
    if score <= NONE {
        return None;
    }
    let mut positions = vec![0; n];
    let mut j = end;
    for i in (0..n).rev() {
        positions[i] = j;
        j = from[i * m + j];
    }
    let mut score = score;
    if positions[0] == 0 && hay.len() >= n && hay[..n] == *needle {
        score += BONUS_PREFIX;
    }
    Some(Match { score, positions })
}

fn is_subsequence(needle: &[char], hay: &[char]) -> bool {
    let mut it = hay.iter();
    needle.iter().all(|c| it.any(|h| h == c))
}

/// Byte ranges of `label` covering the matched character `positions`,
/// merged into runs (for highlighting).
pub fn byte_ranges(label: &str, positions: &[usize]) -> Vec<std::ops::Range<usize>> {
    let mut out: Vec<std::ops::Range<usize>> = Vec::new();
    let mut want = positions.iter().copied().peekable();
    for (ci, (bi, ch)) in label.char_indices().enumerate() {
        if want.peek() != Some(&ci) {
            continue;
        }
        want.next();
        let r = bi..bi + ch.len_utf8();
        match out.last_mut() {
            Some(last) if last.end == r.start => last.end = r.end,
            _ => out.push(r),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(q: &str, label: &str) -> i32 {
        fuzzy_match(q, label).unwrap_or_else(|| panic!("{q:?} should match {label:?}")).score
    }

    /// `labels` sorted best first for `q` (non-matches dropped).
    fn rank<'a>(q: &str, labels: &[&'a str]) -> Vec<&'a str> {
        let mut v: Vec<(i32, &str)> = labels.iter().filter_map(|l| fuzzy_match(q, l).map(|m| (m.score, *l))).collect();
        v.sort_by_key(|&(s, l)| (std::cmp::Reverse(s), l));
        v.into_iter().map(|(_, l)| l).collect()
    }

    #[test]
    fn subsequence_required() {
        assert!(fuzzy_match("xyz", "Open settings").is_none());
        assert!(fuzzy_match("sto", "Open settings").is_none(), "order matters within a word");
        assert!(fuzzy_match("opn", "Open settings").is_some());
        assert!(fuzzy_match("OPEN", "open settings").is_some(), "case-insensitive");
    }

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(fuzzy_match("", "anything"), Some(Match { score: 0, positions: vec![] }));
        assert_eq!(fuzzy_match("   ", "anything").unwrap().score, 0);
    }

    #[test]
    fn words_match_in_any_order() {
        let m = fuzzy_match("settings open", "Settings: Open settings").unwrap();
        assert!(m.positions.contains(&10), "{m:?}"); // 'O' of Open
        assert!(fuzzy_match("shell rel", "Shell: Reload running shells").is_some());
        assert!(fuzzy_match("rel shell", "Shell: Reload running shells").is_some());
        assert!(fuzzy_match("rel zzz", "Shell: Reload running shells").is_none(), "every word must match");
    }

    #[test]
    fn positions_point_at_the_best_alignment() {
        // "os" → O(pen) S(ettings), not the s's inside words.
        assert_eq!(fuzzy_match("os", "Open settings").unwrap().positions, [0, 5]);
        // A contiguous run beats scattered letters.
        assert_eq!(fuzzy_match("set", "Show recent: settings").unwrap().positions, [13, 14, 15]);
    }

    #[test]
    fn word_starts_beat_inner_letters() {
        assert!(score("os", "Open settings") > score("os", "Close settings"));
        assert_eq!(rank("rs", &["Terminal: Paste", "Shell: Reload running shells", "Users"])[0], "Shell: Reload running shells");
        assert_eq!(rank("ks", &["Settings: Keyboard shortcuts", "Tasks"])[0], "Settings: Keyboard shortcuts");
    }

    #[test]
    fn camel_case_and_digits_are_boundaries() {
        assert!(score("ms", "openMySettings") > score("ms", "dimsum"));
        assert_eq!(fuzzy_match("1t", "base16Theme").unwrap().positions, [4, 6]);
    }

    #[test]
    fn exact_and_prefix_win() {
        let labels = ["Terminal: Paste", "Settings: Appearance", "Paste"];
        assert_eq!(rank("paste", &labels)[0], "Paste");
        assert!(score("copy", "Copy the selection") > score("copy", "Terminal: Copy"));
        assert!(score("cop", "Terminal: Copy") > score("cop", "Show color picker"));
    }

    #[test]
    fn consecutive_beats_gaps() {
        assert!(score("font", "Bigger font") > score("font", "fold on things"));
        assert!(score("zoom", "Zoom in") > score("zoom", "Zero out of mind"));
    }

    #[test]
    fn earlier_is_better_all_else_equal() {
        assert!(score("x", "x---------") > score("x", "--------- x"));
    }

    #[test]
    fn unicode_labels() {
        let m = fuzzy_match("über", "Öffnen: Über goatty").unwrap();
        assert_eq!(m.positions, [8, 9, 10, 11]);
        assert_eq!(byte_ranges("Öffnen: Über goatty", &m.positions), vec![9..14]);
        assert!(fuzzy_match("ö", "Öffnen").is_some());
    }

    #[test]
    fn byte_ranges_merge_runs() {
        assert_eq!(byte_ranges("Open settings", &[0, 1, 5]), [0..2, 5..6]);
        assert_eq!(byte_ranges("abc", &[]), Vec::<std::ops::Range<usize>>::new());
    }

    #[test]
    fn scoring_is_fast() {
        // 500 commands, a 6-letter query: the palette re-ranks on every keystroke.
        let labels: Vec<String> =
            (0..500).map(|i| format!("Category {i}: Do the thing number {i} with some longer description text")).collect();
        let start = std::time::Instant::now();
        let mut hits = 0;
        for _ in 0..10 {
            for l in &labels {
                hits += usize::from(fuzzy_match("thnum desc", l).is_some());
            }
        }
        let per_pass = start.elapsed() / 10;
        assert_eq!(hits, 5000);
        eprintln!("fuzzy: {per_pass:?} per 500 labels");
        // Generous so slow CI machines pass; ~1 ms on a desktop.
        assert!(per_pass < std::time::Duration::from_millis(20), "{per_pass:?} per 500 labels");
    }
}
