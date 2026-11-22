use crate::hotwords::{normalize_word, parse_hotwords};

/// Пост-обработка готового текста: пара «услышано => термин» должна работать
/// и без CTC, и на Whisper, где акустического словаря нет. Замены идут только
/// по явно заданным парам и не задевают сами термины.
pub struct Dictionary {
    aliases: Vec<Alias>,
    protected: Vec<Vec<String>>,
}

struct Alias {
    words: Vec<String>,
    target: String,
}

impl Dictionary {
    pub fn new(raw: &[String]) -> Self {
        let entries = parse_hotwords(raw);
        let mut aliases = Vec::new();
        let mut protected = Vec::new();
        for entry in &entries {
            let term = words_of(&entry.to_display);
            if !term.is_empty() {
                protected.push(term.clone());
            }
            let Some(from) = &entry.from else {
                continue;
            };
            let words: Vec<String> = from.iter().flat_map(|part| words_of(part)).collect();
            // Односимвольные пары давали бы замены на каждом предлоге.
            if words.iter().map(|word| word.chars().count()).sum::<usize>() < 2
                || words.join(" ") == term.join(" ")
            {
                continue;
            }
            aliases.push(Alias {
                words,
                target: entry.to_display.clone(),
            });
        }
        aliases.sort_by(|left, right| {
            right
                .words
                .len()
                .cmp(&left.words.len())
                .then(right.words.join(" ").len().cmp(&left.words.join(" ").len()))
                .then(left.words.join(" ").cmp(&right.words.join(" ")))
        });
        Self { aliases, protected }
    }

    pub fn alias_count(&self) -> usize {
        self.aliases.len()
    }

    pub fn term_count(&self) -> usize {
        self.protected.len()
    }

    pub fn apply(&self, text: &mut String) -> usize {
        if self.aliases.is_empty() {
            return 0;
        }
        let tokens = words_with_spans(text);
        if tokens.is_empty() {
            return 0;
        }
        let protected = self.protected_mask(&tokens);
        let mut out = String::with_capacity(text.len());
        let mut copied = 0;
        let mut replaced = 0;
        let mut index = 0;
        while index < tokens.len() {
            if protected[index] {
                index += 1;
                continue;
            }
            let Some(alias) = self.aliases.iter().find(|alias| {
                let end = index + alias.words.len();
                end <= tokens.len()
                    && !protected[index..end].iter().any(|flag| *flag)
                    && tokens[index..end]
                        .iter()
                        .zip(&alias.words)
                        .all(|(token, word)| token.word == *word)
                    && (alias.words.len() == 1
                        || text[tokens[index].end..tokens[end - 1].start]
                            .chars()
                            .all(|character| !character.is_alphanumeric()))
            }) else {
                index += 1;
                continue;
            };
            out.push_str(&text[copied..tokens[index].start]);
            out.push_str(&alias.target);
            copied = tokens[index + alias.words.len() - 1].end;
            replaced += alias.words.len();
            index += alias.words.len();
        }
        if replaced == 0 {
            return 0;
        }
        out.push_str(&text[copied..]);
        *text = out;
        replaced
    }

    fn protected_mask(&self, tokens: &[Token]) -> Vec<bool> {
        let mut mask = vec![false; tokens.len()];
        for term in &self.protected {
            if term.is_empty() || term.len() > tokens.len() {
                continue;
            }
            for start in 0..=tokens.len() - term.len() {
                if tokens[start..start + term.len()]
                    .iter()
                    .zip(term)
                    .all(|(token, word)| token.word == *word)
                {
                    mask[start..start + term.len()].fill(true);
                }
            }
        }
        mask
    }
}

struct Token {
    start: usize,
    end: usize,
    word: String,
}

fn words_with_spans(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        while let Some(character) = text[offset..].chars().next() {
            if character.is_whitespace() {
                offset += character.len_utf8();
            } else {
                break;
            }
        }
        if offset >= text.len() {
            break;
        }
        let start = offset;
        while let Some(character) = text[offset..].chars().next() {
            if character.is_whitespace() {
                break;
            }
            offset += character.len_utf8();
        }
        let raw = &text[start..offset];
        let Some((begin, _)) = raw
            .char_indices()
            .find(|(_, character)| character.is_alphanumeric())
        else {
            continue;
        };
        let Some((last, character)) = raw
            .char_indices()
            .rev()
            .find(|(_, character)| character.is_alphanumeric())
        else {
            continue;
        };
        tokens.push(Token {
            start: start + begin,
            end: start + last + character.len_utf8(),
            word: normalize_word(&raw[begin..last + character.len_utf8()]),
        });
    }
    tokens
}

fn words_of(value: &str) -> Vec<String> {
    words_with_spans(value)
        .into_iter()
        .map(|token| token.word)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dictionary(raw: &[&str]) -> Dictionary {
        Dictionary::new(
            &raw.iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>(),
        )
    }

    fn corrected(dictionary: &Dictionary, text: &str) -> String {
        let mut text = text.to_string();
        dictionary.apply(&mut text);
        text
    }

    #[test]
    fn replaces_aliases_by_words_and_keeps_punctuation() {
        let dictionary = dictionary(&["мерши => merge"]);
        assert_eq!(
            corrected(&dictionary, "Мы обсудили мерши, и всё."),
            "Мы обсудили merge, и всё."
        );
        assert_eq!(corrected(&dictionary, "МЕРШИ!"), "merge!");
        assert_eq!(corrected(&dictionary, "мершинг"), "мершинг");
    }

    #[test]
    fn replaces_multi_word_aliases_only_with_spaces_between() {
        let dictionary = dictionary(&["юзер флоу => User Flow"]);
        assert_eq!(
            corrected(&dictionary, "юзер  флоу готов"),
            "User Flow готов"
        );
        assert_eq!(corrected(&dictionary, "юзер-флоу готов"), "юзер-флоу готов");
    }

    #[test]
    fn protects_terms_from_other_aliases() {
        let dictionary = dictionary(&["Сбер Банк", "сбер => Сбербанк"]);
        assert_eq!(
            corrected(&dictionary, "Сбер Банк и сбер"),
            "Сбер Банк и Сбербанк"
        );
    }

    #[test]
    fn skips_empty_short_and_equal_aliases() {
        let dictionary = dictionary(&["я => I", "merge => merge", " => пусто"]);
        assert_eq!(dictionary.alias_count(), 0);
        assert_eq!(corrected(&dictionary, "я merge"), "я merge");
    }

    #[test]
    fn case_only_pair_stays_listen_only() {
        let dictionary = dictionary(&["api => API"]);
        assert_eq!(dictionary.alias_count(), 0);
        assert_eq!(corrected(&dictionary, "api"), "api");
    }

    #[test]
    fn hyphenated_alias_matches_one_token() {
        let dictionary = dictionary(&["секс-баг => фикс багов"]);
        assert_eq!(corrected(&dictionary, "секс-баг готов"), "фикс багов готов");
    }

    #[test]
    fn normalized_words_match_case_and_yo() {
        let dictionary = dictionary(&["ёлка => Ёлочка"]);
        assert_eq!(corrected(&dictionary, "ЁЛКА стоит"), "Ёлочка стоит");
    }

    #[test]
    fn longest_alias_wins_and_outputs_are_not_rescanned() {
        let dictionary = dictionary(&["альфа => бета", "альфарий => гамма"]);
        assert_eq!(corrected(&dictionary, "альфарий альфа"), "гамма бета");
    }
}
