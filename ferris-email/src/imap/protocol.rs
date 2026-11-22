//! Literal-aware response parser for the IMAP subset this crate speaks.
//!
//! An IMAP response is not a line: an untagged `FETCH` carries a literal
//! (`{123}\r\n<123 bytes>`), so a naive line splitter would hand the message
//! body to the next command's parser. [`Response`] is therefore assembled from
//! tokens, and only [`literal_marker`] decides where the protocol pauses for a
//! literal. Everything else — parents, atoms, the search number list — is
//! parsed from the token stream, which keeps the parser independent of the
//! transport and testable without a socket.

/// One lexical unit of an IMAP response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// A bare atom or a quoted string, quotes already removed.
    Atom(String),
    /// The bytes of a `{n}` literal.
    Literal(Vec<u8>),
    /// A literal larger than the configured message limit. The bytes were
    /// drained from the connection so the next response starts in sync, but
    /// they are not kept in memory.
    LiteralTooLarge(u64),
    /// `(`
    LParen,
    /// `)`
    RParen,
}

/// The three completed-response shapes the client consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// `* …`
    Untagged(Vec<Token>),
    /// `<tag> OK|NO|BAD text`
    Tagged {
        tag: String,
        status: Status,
        tokens: Vec<Token>,
    },
    /// `+ text` — only expected for a non-synchronizing literal, which this
    /// client does not send.
    Continuation(String),
}

/// Tagged status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    No,
    Bad,
}

/// A `{n}` literal marker at the end of `line`.
///
/// Returns the byte offset of the `{` and the declared length. IMAP requires
/// the marker to be the last thing before CRLF; a `+` (non-synchronizing
/// literal) is accepted because servers may echo it.
#[must_use]
pub fn literal_marker(line: &str) -> Option<(usize, u64)> {
    let rest = line.trim_end_matches(['\r', '\n']);
    let start = rest.rfind('{')?;
    let body = rest.get(start + 1..)?.strip_suffix('}')?;
    let digits = body.strip_suffix('+').unwrap_or(body);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().map(|len| (start, len))
}

/// Tokenize the text portion of a response line, appending to `out`.
///
/// Literal markers are stripped by the caller before this runs, so `{` and `}`
/// are ordinary atom characters. The tokenizer is byte-oriented on purpose:
/// IMAP atoms are ASCII and a non-UTF-8 byte in a header must not abort the
/// response.
pub fn tokenize_into(line: &str, out: &mut Vec<Token>) {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b' ' | b'\t' => index += 1,
            b'(' => {
                out.push(Token::LParen);
                index += 1;
            }
            b')' => {
                out.push(Token::RParen);
                index += 1;
            }
            _ => {
                let start = index;
                while index < bytes.len() && !matches!(bytes[index], b' ' | b'\t' | b'(' | b')') {
                    index += 1;
                }
                out.push(Token::Atom(
                    String::from_utf8_lossy(&bytes[start..index]).into_owned(),
                ));
            }
        }
    }
}

/// Classify an assembled token stream as a completed response.
///
/// # Errors
///
/// Fails on an empty stream, a tagged response without `OK`/`NO`/`BAD`, or an
/// untagged/tagged prefix the client cannot interpret.
pub fn classify(tokens: &[Token]) -> Result<Response, String> {
    let Some(Token::Atom(first)) = tokens.first() else {
        return Err("response starts with a non-atom token".to_string());
    };
    if first == "+" {
        let text = atoms(&tokens[1..]).join(" ");
        return Ok(Response::Continuation(text));
    }
    if first == "*" {
        return Ok(Response::Untagged(tokens[1..].to_vec()));
    }
    let Some(Token::Atom(status)) = tokens.get(1) else {
        return Err(format!("tagged response `{first}` has no status"));
    };
    let status = match status.to_ascii_uppercase().as_str() {
        "OK" => Status::Ok,
        "NO" => Status::No,
        "BAD" => Status::Bad,
        other => return Err(format!("unknown tagged status `{other}`")),
    };
    Ok(Response::Tagged {
        tag: first.clone(),
        status,
        tokens: tokens[2..].to_vec(),
    })
}

/// The atoms of a token slice, ignoring literals and parentheses.
#[must_use]
pub fn atoms(tokens: &[Token]) -> Vec<String> {
    tokens
        .iter()
        .filter_map(|token| match token {
            Token::Atom(atom) => Some(atom.clone()),
            _ => None,
        })
        .collect()
}

/// Parse `SEARCH` UIDs from an untagged response.
///
/// `* SEARCH 4 9 12` yields `[4, 9, 12]`; an empty search yields an empty list.
#[must_use]
pub fn parse_search(tokens: &[Token]) -> Vec<u32> {
    let some_search = tokens.iter().position(
        |token| matches!(token, Token::Atom(atom) if atom.eq_ignore_ascii_case("SEARCH")),
    );
    let Some(start) = some_search else {
        return Vec::new();
    };
    tokens[start + 1..]
        .iter()
        .filter_map(|token| match token {
            Token::Atom(atom) => atom.parse().ok(),
            _ => None,
        })
        .collect()
}

/// One untagged `FETCH` response, reduced to what the poller needs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FetchData {
    pub uid: Option<u32>,
    pub message: Option<Vec<u8>>,
    pub too_large: Option<u64>,
}

/// Parse an untagged `FETCH` response.
///
/// Returns `None` when the token stream is not a `FETCH` (the caller skips it,
/// since servers may also send `EXISTS`/`RECENT`/`FLAGS` while a fetch is in
/// flight).
///
/// # Errors
///
/// A malformed FETCH (a body field without a literal, a non-numeric UID) is
/// reported instead of being read as "no message".
pub fn parse_fetch(tokens: &[Token]) -> Result<Option<FetchData>, String> {
    let Some(fetch) = tokens
        .iter()
        .position(|token| matches!(token, Token::Atom(atom) if atom.eq_ignore_ascii_case("FETCH")))
    else {
        return Ok(None);
    };
    let mut data = FetchData::default();
    let mut index = fetch + 1;
    while index < tokens.len() {
        let Some(Token::Atom(field)) = tokens.get(index) else {
            index += 1;
            continue;
        };
        match field.to_ascii_uppercase().as_str() {
            "UID" => {
                let Some(Token::Atom(value)) = tokens.get(index + 1) else {
                    return Err("FETCH UID without a value".to_string());
                };
                data.uid = Some(
                    value
                        .parse()
                        .map_err(|_| format!("FETCH UID `{value}` is not a number"))?,
                );
                index += 2;
            }
            // A server answers `BODY.PEEK[]` with `BODY[]` per RFC 3501, but
            // some echo the requested item verbatim; accepting both keeps the
            // non-flagging fetch working either way (MAIL-01). `RFC822` stays
            // for servers that send it in an untagged response we did not ask
            // for.
            "RFC822" | "BODY[]" | "BODY" | "BODY.PEEK[]" => {
                match tokens.get(index + 1) {
                    Some(Token::Literal(bytes)) => data.message = Some(bytes.clone()),
                    Some(Token::LiteralTooLarge(size)) => data.too_large = Some(*size),
                    _ => return Err(format!("FETCH {field} without a literal")),
                }
                index += 2;
            }
            "FLAGS" => {
                index += 1;
                if matches!(tokens.get(index), Some(Token::LParen)) {
                    index = skip_group(tokens, index);
                }
            }
            _ => index += 1,
        }
    }
    Ok(Some(data))
}

/// Index just past the parenthesized group starting at `start`.
const fn skip_group(tokens: &[Token], start: usize) -> usize {
    let mut depth = 0_usize;
    let mut index = start;
    while index < tokens.len() {
        match tokens[index] {
            Token::LParen => depth += 1,
            Token::RParen => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return index + 1;
                }
            }
            _ => {}
        }
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_literal_marker_is_only_recognized_at_the_end_of_a_line() {
        assert_eq!(literal_marker("* 1 FETCH (RFC822 {12}"), Some((18, 12)));
        assert_eq!(literal_marker("* 1 FETCH (RFC822 {12+}"), Some((18, 12)));
        assert_eq!(literal_marker("* OK no literal here"), None);
        assert_eq!(literal_marker("* 1 FETCH (X {abc}"), None);
        assert_eq!(literal_marker("* 1 FETCH (X {}"), None);
    }

    #[test]
    fn tokenizing_splits_atoms_parentheses_and_whitespace() {
        let mut tokens = Vec::new();
        tokenize_into("* 3 FETCH (UID 42 FLAGS (\\Seen))", &mut tokens);
        assert_eq!(
            tokens,
            vec![
                Token::Atom("*".to_string()),
                Token::Atom("3".to_string()),
                Token::Atom("FETCH".to_string()),
                Token::LParen,
                Token::Atom("UID".to_string()),
                Token::Atom("42".to_string()),
                Token::Atom("FLAGS".to_string()),
                Token::LParen,
                Token::Atom("\\Seen".to_string()),
                Token::RParen,
                Token::RParen,
            ]
        );
    }

    #[test]
    fn classify_distinguishes_untagged_tagged_and_continuation() {
        let untagged =
            classify(&[Token::Atom("*".into()), Token::Atom("OK".into())]).expect("untagged");
        assert!(matches!(untagged, Response::Untagged(_)));

        let tagged = classify(&[
            Token::Atom("A1".into()),
            Token::Atom("NO".into()),
            Token::Atom("[AUTHENTICATIONFAILED]".into()),
        ])
        .expect("tagged");
        match tagged {
            Response::Tagged { tag, status, .. } => {
                assert_eq!(tag, "A1");
                assert_eq!(status, Status::No);
            }
            other => panic!("expected tagged, got {other:?}"),
        }

        let continuation = classify(&[Token::Atom("+".into()), Token::Atom("Ready".into())])
            .expect("continuation");
        assert_eq!(continuation, Response::Continuation("Ready".to_string()));
    }

    #[test]
    fn search_lists_uids_and_ignores_an_empty_mailbox() {
        let tokens = vec![
            Token::Atom("*".into()),
            Token::Atom("SEARCH".into()),
            Token::Atom("4".into()),
            Token::Atom("9".into()),
        ];
        assert_eq!(parse_search(&tokens), vec![4, 9]);
        let empty = vec![Token::Atom("*".into()), Token::Atom("SEARCH".into())];
        assert!(parse_search(&empty).is_empty());
    }

    #[test]
    fn fetch_reads_uid_message_and_skips_a_flags_group() {
        let tokens = vec![
            Token::Atom("*".into()),
            Token::Atom("1".into()),
            Token::Atom("FETCH".into()),
            Token::LParen,
            Token::Atom("UID".into()),
            Token::Atom("42".into()),
            Token::Atom("FLAGS".into()),
            Token::LParen,
            Token::Atom("\\Seen".into()),
            Token::RParen,
            Token::Atom("RFC822".into()),
            Token::Literal(b"Subject: hi\r\n\r\nbody".to_vec()),
            Token::RParen,
        ];
        let data = parse_fetch(&tokens)
            .expect("parse")
            .expect("a fetch response");
        assert_eq!(data.uid, Some(42));
        assert_eq!(
            data.message.as_deref(),
            Some(b"Subject: hi\r\n\r\nbody".as_slice())
        );
    }

    #[test]
    fn an_oversized_literal_is_reported_without_its_bytes() {
        let tokens = vec![
            Token::Atom("*".into()),
            Token::Atom("1".into()),
            Token::Atom("FETCH".into()),
            Token::LParen,
            Token::Atom("UID".into()),
            Token::Atom("7".into()),
            Token::Atom("RFC822".into()),
            Token::LiteralTooLarge(90_000_000),
            Token::RParen,
        ];
        let data = parse_fetch(&tokens).expect("parse").expect("fetch");
        assert_eq!(data.uid, Some(7));
        assert_eq!(data.too_large, Some(90_000_000));
        assert!(data.message.is_none());
    }

    #[test]
    fn a_fetch_without_a_literal_is_an_error() {
        let tokens = vec![
            Token::Atom("*".into()),
            Token::Atom("1".into()),
            Token::Atom("FETCH".into()),
            Token::LParen,
            Token::Atom("RFC822".into()),
            Token::RParen,
        ];
        assert!(parse_fetch(&tokens).is_err());
    }

    #[test]
    fn a_peek_fetch_is_recognised() {
        // The poller asks for `BODY.PEEK[]` so the server does not set
        // `\Seen`; a server echoing the requested item must still parse.
        let body = b"Subject: hi\r\n\r\nbody";
        let tokens = vec![
            Token::Atom("*".into()),
            Token::Atom("1".into()),
            Token::Atom("FETCH".into()),
            Token::LParen,
            Token::Atom("UID".into()),
            Token::Atom("9".into()),
            Token::Atom("BODY.PEEK[]".into()),
            Token::Literal(body.to_vec()),
            Token::RParen,
        ];
        let data = parse_fetch(&tokens)
            .expect("parse")
            .expect("a fetch response");
        assert_eq!(data.uid, Some(9));
        assert_eq!(data.message.as_deref(), Some(body.as_slice()));
    }
}
