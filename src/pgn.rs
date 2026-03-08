use std::io::Read;

use memchr::{memchr, memchr3};

// ─── Constants ───────────────────────────────────────────────────────────────

const DEFAULT_CHUNK: usize = 8192 * 4;
const MAX_TOKEN: usize = 255;

// ─── TokenBuf ────────────────────────────────────────────────────────────────

struct TokenBuf {
    data: [u8; MAX_TOKEN],
    len: usize,
}

impl TokenBuf {
    fn new() -> Self {
        Self {
            data: [0; MAX_TOKEN],
            len: 0,
        }
    }
    fn is_empty(&self) -> bool {
        self.len == 0
    }
    fn clear(&mut self) {
        self.len = 0;
    }
    fn push(&mut self, b: u8) -> bool {
        if self.len < MAX_TOKEN {
            self.data[self.len] = b;
            self.len += 1;
            true
        } else {
            false
        }
    }
    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.data[..self.len]).unwrap_or("")
    }
}

// ─── Reader ───────────────────────────────────────────────────────────────────
//
// Buffered byte source with a clean peek/bump/next interface.
// Transparently discards `\r` bytes so the rest of the parser is CRLF-agnostic.

struct Reader<R: Read> {
    inner: R,
    buf: Vec<u8>,
    pos: usize,
    len: usize,
}

impl<R: Read> Reader<R> {
    fn new(inner: R, chunk_size: usize) -> Self {
        assert!(chunk_size > 0, "chunk size must be greater than zero");

        Self {
            inner,
            buf: vec![0; chunk_size],
            pos: 0,
            len: 0,
        }
    }

    fn refill(&mut self) -> bool {
        self.pos = 0;
        match self.inner.read(self.buf.as_mut()) {
            Ok(n) if n > 0 => {
                self.len = n;
                true
            }
            _ => {
                self.len = 0;
                false
            }
        }
    }

    /// Current byte, skipping `\r`. Returns `None` at EOF.
    fn peek(&mut self) -> Option<u8> {
        loop {
            if self.pos < self.len {
                let b = self.buf[self.pos];
                if b == b'\r' {
                    self.pos += 1;
                    continue;
                }
                return Some(b);
            }
            if !self.refill() {
                return None;
            }
        }
    }

    /// Advance past the current byte (always call `peek` first).
    #[inline]
    fn bump(&mut self) {
        self.pos += 1;
    }

    /// `peek` + `bump`.
    #[inline]
    fn next(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.bump();
        Some(b)
    }

    fn skip_while(&mut self, pred: impl Fn(u8) -> bool) {
        while let Some(b) = self.peek() {
            if pred(b) {
                self.bump();
            } else {
                break;
            }
        }
    }

    // ── Structural skippers ──────────────────────────────────────────────────

    /// Drain text of `{ … }` into `out`. Called after the opening `{`.
    fn drain_comment(&mut self, out: &mut String) {
        loop {
            let slice = &self.buf[self.pos..self.len];
            match memchr(b'}', slice) {
                Some(i) => {
                    for &b in &slice[..i] {
                        if b != b'\r' {
                            out.push(b as char);
                        }
                    }
                    self.pos += i + 1;
                    return;
                }
                None => {
                    for &b in slice {
                        if b != b'\r' {
                            out.push(b as char);
                        }
                    }
                    self.pos = self.len;
                    if !self.refill() {
                        return;
                    }
                }
            }
        }
    }

    /// Skip `( … )` variation with nesting and embedded `{ }`. Called after `(`.
    fn skip_variation(&mut self) {
        let mut depth = 1usize;
        loop {
            let slice = &self.buf[self.pos..self.len];
            match memchr3(b'(', b')', b'{', slice) {
                Some(i) => {
                    let b = slice[i];
                    self.pos += i + 1;
                    match b {
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                return;
                            }
                        }
                        // skip embedded { comment }
                        _ => loop {
                            let inner = &self.buf[self.pos..self.len];
                            match memchr(b'}', inner) {
                                Some(j) => {
                                    self.pos += j + 1;
                                    break;
                                }
                                None => {
                                    self.pos = self.len;
                                    if !self.refill() {
                                        return;
                                    }
                                }
                            }
                        },
                    }
                }
                None => {
                    self.pos = self.len;
                    if !self.refill() {
                        return;
                    }
                }
            }
        }
    }

    /// Skip to (but not past) the next `\n`. Uses `memchr` for bulk scanning.
    fn skip_to_newline(&mut self) {
        loop {
            let slice = &self.buf[self.pos..self.len];
            match memchr(b'\n', slice) {
                Some(i) => {
                    self.pos += i;
                    return;
                }
                None => {
                    self.pos = self.len;
                    if !self.refill() {
                        return;
                    }
                }
            }
        }
    }

    /// Advance `pos` to the next `[` in the stream. Returns `false` on EOF.
    fn skip_to_open_bracket(&mut self) -> bool {
        // `bump()` may have advanced pos past len; normalize before raw buffer access.
        if self.pos > self.len {
            self.pos = self.len;
        }
        loop {
            let slice = &self.buf[self.pos..self.len];
            match memchr(b'[', slice) {
                Some(i) => {
                    self.pos += i;
                    return true;
                }
                None => {
                    self.pos = self.len;
                    if !self.refill() {
                        return false;
                    }
                }
            }
        }
    }

    /// Skip `$NNN` NAG digits. Called after `$`.
    fn skip_nag(&mut self) {
        self.skip_while(|b| b.is_ascii_digit());
    }
}

// ─── Error ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamParserErrorCode {
    None,
    ExceededMaxStringLength,
    InvalidHeaderMissingClosingBracket,
    InvalidHeaderMissingClosingQuote,
    NotEnoughData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamParserError(pub StreamParserErrorCode);

impl StreamParserError {
    pub fn none() -> Self {
        StreamParserError(StreamParserErrorCode::None)
    }
    pub fn has_error(&self) -> bool {
        self.0 != StreamParserErrorCode::None
    }
    pub fn message(&self) -> &'static str {
        match self.0 {
            StreamParserErrorCode::None => "No error",
            StreamParserErrorCode::ExceededMaxStringLength => "Exceeded max string length",
            StreamParserErrorCode::InvalidHeaderMissingClosingBracket => {
                "Invalid header: missing closing bracket"
            }
            StreamParserErrorCode::InvalidHeaderMissingClosingQuote => {
                "Invalid header: missing closing quote"
            }
            StreamParserErrorCode::NotEnoughData => "Not enough data",
        }
    }
}

impl std::fmt::Display for StreamParserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}
impl std::error::Error for StreamParserError {}

// ─── Visitor ─────────────────────────────────────────────────────────────────

/// Callback-based interface for receiving PGN events.
pub trait Visitor {
    /// Called when a new game begins (before the first header).
    fn start_pgn(&mut self);

    /// Called for each `[Key "Value"]` header tag.
    fn header(&mut self, key: &str, value: &str);

    /// Called once all headers have been read and moves are about to start.
    fn start_moves(&mut self);

    /// Called for each move token. `comment` is the text of any `{ }` that
    /// immediately follows the move (empty if none).
    fn move_token(&mut self, mv: &str, comment: &str);

    /// Called when the game ends (after the termination symbol or EOF).
    fn end_pgn(&mut self);

    /// Set the skip flag to avoid receiving move events for the current game.
    /// `end_pgn` is always called regardless.
    fn skip_pgn(&mut self, _skip: bool) {}

    /// Returns `true` if the current game should be skipped.
    fn skip(&self) -> bool {
        false
    }
}

// ─── StreamParser ────────────────────────────────────────────────────────────

/// Streaming PGN parser. Reads from any [`std::io::Read`] source and fires
/// [`Visitor`] callbacks. Handles multiple games and all standard termination
/// symbols (`1-0`, `0-1`, `1/2-1/2`, `*`).
pub struct StreamParser<R: Read> {
    reader: Reader<R>,
}

impl<R: Read> StreamParser<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader: Reader::new(reader, DEFAULT_CHUNK),
        }
    }

    pub fn with_chunk_size(reader: R, chunk_size: usize) -> Self {
        Self {
            reader: Reader::new(reader, chunk_size),
        }
    }

    /// Parse all games, calling `vis` for every event.
    pub fn read_games<V: Visitor>(&mut self, vis: &mut V) -> Result<(), StreamParserError> {
        if !self.reader.refill() {
            return Err(StreamParserError(StreamParserErrorCode::NotEnoughData));
        }

        loop {
            // Advance to the opening '[' of the next game.
            if !self.reader.skip_to_open_bracket() {
                return Ok(());
            }

            vis.skip_pgn(false);
            vis.start_pgn();

            self.parse_headers(vis)?;

            if !vis.skip() {
                vis.start_moves();
            }

            self.parse_moves(vis)?;

            vis.end_pgn();
            vis.skip_pgn(false);
        }
    }

    // ── Header parsing ───────────────────────────────────────────────────────
    //
    // Reads zero or more `[TagName "TagValue"]` lines until a blank line or a
    // line that does not begin with `[`.

    fn parse_headers<V: Visitor>(&mut self, vis: &mut V) -> Result<(), StreamParserError> {
        let mut key = TokenBuf::new();
        let mut val = TokenBuf::new();

        loop {
            self.reader.skip_while(|b| matches!(b, b' ' | b'\t'));

            match self.reader.peek() {
                None => return Ok(()),
                Some(b'\n') => {
                    self.reader.bump();
                    return Ok(());
                } // blank line
                Some(b'[') => {
                    self.reader.bump();
                }
                _ => return Ok(()), // non-tag → start of moves
            }

            // Tag name: everything up to whitespace
            while let Some(b) = self.reader.peek() {
                if is_ws(b) {
                    break;
                }
                if !key.push(b) {
                    return Err(StreamParserError(
                        StreamParserErrorCode::ExceededMaxStringLength,
                    ));
                }
                self.reader.bump();
            }
            self.reader.skip_while(|b| matches!(b, b' ' | b'\t'));

            // Opening quote
            if self.reader.peek() != Some(b'"') {
                self.reader.skip_to_newline();
                key.clear();
                val.clear();
                continue;
            }
            self.reader.bump();

            // Tag value with backslash-escape support
            let mut backslash = false;
            loop {
                match self.reader.next() {
                    None | Some(b'\n') => {
                        return Err(StreamParserError(
                            StreamParserErrorCode::InvalidHeaderMissingClosingQuote,
                        ));
                    }
                    Some(b'\\') if !backslash => {
                        backslash = true;
                    }
                    Some(b'"') if !backslash => break,
                    Some(b) => {
                        backslash = false;
                        if !val.push(b) {
                            return Err(StreamParserError(
                                StreamParserErrorCode::ExceededMaxStringLength,
                            ));
                        }
                    }
                }
            }

            // Closing ']'
            self.reader.skip_while(|b| matches!(b, b' ' | b'\t'));
            if self.reader.peek() == Some(b']') {
                self.reader.bump();
            } else {
                return Err(StreamParserError(
                    StreamParserErrorCode::InvalidHeaderMissingClosingBracket,
                ));
            }

            if !vis.skip() {
                vis.header(key.as_str(), val.as_str());
            }
            key.clear();
            val.clear();

            // Skip remainder of the tag line
            self.reader.skip_to_newline();
            if self.reader.peek() == Some(b'\n') {
                self.reader.bump();
            }
        }
    }

    // ── Move parsing ─────────────────────────────────────────────────────────
    //
    // Dispatches on the first byte of each token:
    //   digit      → move number  (N...N '.')  or termination  (1-0 / 0-1 / 1/2-1/2)
    //   '*'        → bare game result
    //   '['        → start of next game
    //   '{' '(' '$'→ comment / variation / NAG
    //   else       → SAN move token

    fn parse_moves<V: Visitor>(&mut self, vis: &mut V) -> Result<(), StreamParserError> {
        let mut mv = TokenBuf::new();
        let mut comment = String::new();

        'outer: loop {
            self.reader.skip_while(is_ws);

            let b = match self.reader.peek() {
                None => break,
                Some(b) => b,
            };

            match b {
                b'[' => break, // next game starts

                b'*' => {
                    self.reader.bump();
                    break;
                }

                b'{' => {
                    // stand-alone comment (no preceding move)
                    self.reader.bump();
                    self.reader.drain_comment(&mut comment);
                    if !vis.skip() {
                        vis.move_token("", &comment);
                    }
                    comment.clear();
                }

                b'(' => {
                    self.reader.bump();
                    self.reader.skip_variation();
                }
                b'$' => {
                    self.reader.bump();
                    self.reader.skip_nag();
                }

                b'0'..=b'9' => {
                    let first = b;
                    let first_is_one = first == b'1';
                    let first_is_zero = first == b'0';
                    self.reader.bump();
                    // peek at the character after `first` before consuming more digits
                    let has_second_digit = self.reader.peek().map_or(false, |c| c.is_ascii_digit());
                    self.reader.skip_while(|c| c.is_ascii_digit());

                    match self.reader.peek() {
                        Some(b'.') => {
                            // move number — consume all dots (e.g. "1..." for Black)
                            self.reader.skip_while(|c| c == b'.');
                        }

                        Some(b'-') if first_is_one && !has_second_digit => {
                            // 1-0
                            self.reader.bump(); // '-'
                            self.reader.bump(); // '0'
                            break 'outer;
                        }

                        Some(b'/') if first_is_one && !has_second_digit => {
                            // 1/2-1/2  (6 chars after the leading '1': /2-1/2)
                            for _ in 0..6 {
                                self.reader.bump();
                            }
                            break 'outer;
                        }

                        Some(b'-') if first_is_zero && !has_second_digit => {
                            self.reader.bump(); // consume '-'
                            match self.reader.peek() {
                                Some(b'1') => {
                                    // 0-1
                                    self.reader.bump();
                                    break 'outer;
                                }
                                _ => {
                                    // 0-0 or 0-0-0 castling written with zeros
                                    if !mv.push(b'0') || !mv.push(b'-') {
                                        return Err(StreamParserError(
                                            StreamParserErrorCode::ExceededMaxStringLength,
                                        ));
                                    }
                                    self.read_move_token(&mut mv)?;
                                    self.read_move_appendix(&mut comment);
                                    if !mv.is_empty() && !vis.skip() {
                                        vis.move_token(mv.as_str(), &comment);
                                    }
                                    mv.clear();
                                    comment.clear();
                                }
                            }
                        }

                        _ => {} // stray digits — skip
                    }
                }

                _ => {
                    // Regular SAN move (includes O-O castling, piece moves, …)
                    self.read_move_token(&mut mv)?;
                    self.read_move_appendix(&mut comment);
                    if !mv.is_empty() && !vis.skip() {
                        vis.move_token(mv.as_str(), &comment);
                    }
                    mv.clear();
                    comment.clear();
                }
            }
        }

        // Flush any move that was being assembled when EOF was reached.
        if !mv.is_empty() && !vis.skip() {
            vis.move_token(mv.as_str(), &comment);
        }

        Ok(())
    }

    /// Read a SAN token: bytes until whitespace or a structural character.
    fn read_move_token(&mut self, mv: &mut TokenBuf) -> Result<(), StreamParserError> {
        while let Some(b) = self.reader.peek() {
            if is_ws(b) || matches!(b, b'{' | b'(' | b')' | b'$' | b'[') {
                break;
            }
            if !mv.push(b) {
                return Err(StreamParserError(
                    StreamParserErrorCode::ExceededMaxStringLength,
                ));
            }
            self.reader.bump();
        }
        Ok(())
    }

    /// Consume any sequence of `{comment}`, `(variation)`, `$NAG`, and
    /// whitespace that follows a move token.  Fills `comment` with the text
    /// of the first (or only) `{ }` block.
    fn read_move_appendix(&mut self, comment: &mut String) {
        loop {
            self.reader.skip_while(is_ws);
            match self.reader.peek() {
                Some(b'{') => {
                    self.reader.bump();
                    self.reader.drain_comment(comment);
                }
                Some(b'(') => {
                    self.reader.bump();
                    self.reader.skip_variation();
                }
                Some(b'$') => {
                    self.reader.bump();
                    self.reader.skip_nag();
                }
                _ => break,
            }
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

#[inline]
fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[derive(Default)]
    struct Recorder {
        games: Vec<RecordedGame>,
        skip_flag: bool,
    }

    #[derive(Default, Debug)]
    struct RecordedGame {
        headers: Vec<(String, String)>,
        moves: Vec<(String, String)>,
    }

    impl Visitor for Recorder {
        fn start_pgn(&mut self) {
            println!("start_pgn");
            self.games.push(RecordedGame::default());
        }
        fn header(&mut self, key: &str, value: &str) {
            self.games
                .last_mut()
                .unwrap()
                .headers
                .push((key.to_owned(), value.to_owned()));
        }
        fn start_moves(&mut self) {}
        fn move_token(&mut self, mv: &str, comment: &str) {
            println!("move_token: '{}' with comment '{}'", mv, comment);
            self.games
                .last_mut()
                .unwrap()
                .moves
                .push((mv.to_owned(), comment.to_owned()));
        }
        fn end_pgn(&mut self) {}
        fn skip_pgn(&mut self, skip: bool) {
            self.skip_flag = skip;
        }
        fn skip(&self) -> bool {
            self.skip_flag
        }
    }

    fn parse(pgn: &[u8]) -> Recorder {
        let mut cursor = Cursor::new(pgn);
        let mut vis = Recorder::default();

        StreamParser::with_chunk_size(&mut cursor, 1)
            .read_games(&mut vis)
            .unwrap();
        vis
    }

    fn parse_file(path: &str) -> Recorder {
        let data = std::fs::read(path).expect("failed to read pgn file");
        parse(&data)
    }

    // ── original tests ────────────────────────────────────────────────────────

    #[test]
    fn test_single_game_headers() {
        let pgn = b"[Event \"Test\"]\n[Site \"Local\"]\n\n1. e4 e5 *\n";
        let r = parse(pgn);
        assert_eq!(r.games.len(), 1);
        assert_eq!(r.games[0].headers[0], ("Event".into(), "Test".into()));
        assert_eq!(r.games[0].headers[1], ("Site".into(), "Local".into()));
    }

    #[test]
    fn test_single_game_moves() {
        let pgn = b"[Event \"X\"]\n\n1. e4 e5 2. Nf3 Nc6 *\n";
        let r = parse(pgn);
        // header should be present
        assert_eq!(r.games[0].headers.len(), 1);
        assert_eq!(r.games[0].headers[0], ("Event".into(), "X".into()));
        let moves: Vec<&str> = r.games[0].moves.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(moves, vec!["e4", "e5", "Nf3", "Nc6"]);
    }

    #[test]
    fn test_move_with_comment() {
        let pgn = b"[Event \"X\"]\n\n1. e4 { good move } e5 *\n";
        let r = parse(pgn);
        assert_eq!(r.games[0].headers[0], ("Event".into(), "X".into()));
        assert_eq!(r.games[0].moves[0].0, "e4");
        assert_eq!(r.games[0].moves[0].1, " good move ");
    }

    #[test]
    fn test_termination_1_0() {
        let pgn = b"[Event \"X\"]\n\n1. e4 e5 1-0\n";
        let r = parse(pgn);
        assert_eq!(r.games.len(), 1);
        let moves: Vec<&str> = r.games[0].moves.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(moves, vec!["e4", "e5"]);
        assert_eq!(r.games[0].headers[0], ("Event".into(), "X".into()));
    }

    #[test]
    fn test_termination_0_1() {
        let pgn = b"[Event \"X\"]\n\n1. e4 e5 0-1\n";
        let r = parse(pgn);
        assert_eq!(r.games.len(), 1);
        assert_eq!(r.games[0].headers[0], ("Event".into(), "X".into()));
    }

    #[test]
    fn test_termination_draw() {
        let pgn = b"[Event \"X\"]\n\n1. e4 e5 1/2-1/2\n";
        let r = parse(pgn);
        assert_eq!(r.games.len(), 1);
        let moves: Vec<&str> = r.games[0].moves.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(moves, vec!["e4", "e5"]);
        assert_eq!(r.games[0].headers[0], ("Event".into(), "X".into()));
    }

    #[test]
    fn test_multiple_games() {
        let pgn = b"[Event \"A\"]\n\n1. d4 *\n[Event \"B\"]\n\n1. e4 *\n";
        let r = parse(pgn);
        assert_eq!(r.games.len(), 2);
        assert_eq!(r.games[0].moves[0].0, "d4");
        assert_eq!(r.games[1].moves[0].0, "e4");
        assert_eq!(r.games[0].headers[0], ("Event".into(), "A".into()));
        assert_eq!(r.games[1].headers[0], ("Event".into(), "B".into()));
    }

    #[test]
    fn test_nag_skipped() {
        let pgn = b"[Event \"X\"]\n\n1. e4 $1 e5 *\n";
        let r = parse(pgn);
        let moves: Vec<&str> = r.games[0].moves.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(moves, vec!["e4", "e5"]);
        assert_eq!(r.games[0].headers[0], ("Event".into(), "X".into()));
    }

    #[test]
    fn test_variation_skipped() {
        let pgn = b"[Event \"X\"]\n\n1. e4 (1. d4 d5) e5 *\n";
        let r = parse(pgn);
        let moves: Vec<&str> = r.games[0].moves.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(moves, vec!["e4", "e5"]);
        assert_eq!(r.games[0].headers[0], ("Event".into(), "X".into()));
    }

    #[test]
    fn test_castling_zero_notation() {
        let pgn = b"[Event \"X\"]\n\n1. e4 e5 2. Nf3 Nc6 3. Bc4 Bc5 4. 0-0 0-0 *\n";
        let r = parse(pgn);
        let moves: Vec<&str> = r.games[0].moves.iter().map(|(m, _)| m.as_str()).collect();
        assert!(moves.contains(&"0-0"));
        assert_eq!(r.games[0].headers[0], ("Event".into(), "X".into()));
    }

    #[test]
    fn test_escape_in_header_value() {
        let pgn = b"[Event \"Test\\\"Escaped\"]\n\n*\n";
        let r = parse(pgn);
        assert_eq!(r.games[0].headers[0].1, "Test\"Escaped");
    }

    #[test]
    fn test_basic_pgn() {
        let r = parse_file("./tests/pgns/basic.pgn");
        assert_eq!(r.games.len(), 1);
        let moves = &r.games[0].moves;
        assert_eq!(moves.len(), 130);
        assert_eq!(moves[0].0, "Bg2");
        assert_eq!(moves[0].1, "+1.55/16 0.70s");
        assert_eq!(moves[1].0, "O-O");
        assert_eq!(moves[1].1, "-1.36/18 0.78s");
        assert_eq!(moves[2].0, "O-O");
        assert_eq!(moves[2].1, "+1.84/16 0.42s");
        assert_eq!(moves[3].0, "a5");
        assert_eq!(moves[3].1, "-1.30/16 0.16s");
    }

    #[test]
    fn test_corrupted_pgn() {
        let r = parse_file("./tests/pgns/corrupted.pgn");
        assert_eq!(r.games.len(), 1);
        assert_eq!(r.games[0].moves.len(), 125);
    }

    #[test]
    fn test_no_moves_pgn() {
        let r = parse_file("./tests/pgns/no_moves.pgn");
        assert_eq!(r.games.len(), 1);
        assert_eq!(r.games[0].moves.len(), 0);
    }

    #[test]
    fn test_multiple_pgn() {
        let r = parse_file("./tests/pgns/multiple.pgn");
        assert_eq!(r.games.len(), 4);
    }

    /// The skip visitor skips one game; only one game's moves should be recorded.
    #[test]
    fn test_skip_pgn() {
        use std::io::Cursor;

        struct SkipVisitor {
            inner: Recorder,
            game_idx: usize,
        }

        impl Visitor for SkipVisitor {
            fn start_pgn(&mut self) {
                self.inner.start_pgn();
            }
            fn header(&mut self, key: &str, value: &str) {
                self.inner.header(key, value);
            }
            fn start_moves(&mut self) {
                self.inner.start_moves();
            }
            fn move_token(&mut self, mv: &str, comment: &str) {
                self.inner.move_token(mv, comment);
            }
            fn end_pgn(&mut self) {
                self.game_idx += 1;
                self.inner.end_pgn();
            }
            fn skip_pgn(&mut self, _: bool) {}
            fn skip(&self) -> bool {
                // skip the second game (index 1)
                self.game_idx == 1
            }
        }

        let data = std::fs::read("./tests/pgns/skip.pgn").expect("failed to read skip.pgn");
        let mut cursor = Cursor::new(data);
        let mut vis = SkipVisitor {
            inner: Recorder::default(),
            game_idx: 0,
        };
        StreamParser::new(&mut cursor).read_games(&mut vis).unwrap();

        assert_eq!(vis.inner.games.len(), 2);
        // Only the non-skipped game has 130 moves; the skipped game has 0.
        let total_moves: usize = vis.inner.games.iter().map(|g| g.moves.len()).sum();
        assert_eq!(total_moves, 130);
    }

    #[test]
    fn test_newline_by_moves() {
        let r = parse_file("./tests/pgns/newline.pgn");
        assert_eq!(r.games.len(), 1);
        assert_eq!(r.games[0].moves.len(), 6);
    }

    #[test]
    fn test_castling_0_0_file() {
        let r = parse_file("./tests/pgns/castling.pgn");
        assert_eq!(r.games.len(), 1);
        assert_eq!(r.games[0].moves.len(), 6);
    }

    #[test]
    fn test_black_to_move_castling_0_0_0() {
        let r = parse_file("./tests/pgns/black2move.pgn");
        assert_eq!(r.games.len(), 1);
        assert_eq!(r.games[0].moves.len(), 3);
    }

    #[test]
    fn test_skip_variations_file() {
        let r = parse_file("./tests/pgns/variations.pgn");
        assert_eq!(r.games.len(), 1);
        assert_eq!(r.games[0].moves.len(), 108);
    }

    #[test]
    fn test_read_book() {
        let r = parse_file("./tests/pgns/book.pgn");
        assert_eq!(r.games.len(), 1);
        assert_eq!(r.games[0].moves.len(), 16);
    }

    #[test]
    fn test_no_moves_game_termination_0_1() {
        let r = parse_file("./tests/pgns/no_moves_but_game_termination.pgn");
        assert_eq!(r.games.len(), 1);
        assert_eq!(r.games[0].moves.len(), 0);
    }

    #[test]
    fn test_no_moves_game_termination_draw() {
        let r = parse_file("./tests/pgns/no_moves_but_game_termination_2.pgn");
        assert_eq!(r.games.len(), 1);
        assert_eq!(r.games[0].moves.len(), 0);
    }

    #[test]
    fn test_no_moves_game_termination_asterisk() {
        let r = parse_file("./tests/pgns/no_moves_but_game_termination_3.pgn");
        assert_eq!(r.games.len(), 1);
        assert_eq!(r.games[0].moves.len(), 0);
    }

    #[test]
    fn test_no_moves_game_termination_multiple() {
        let r = parse_file("./tests/pgns/no_moves_but_game_termination_multiple.pgn");
        assert_eq!(r.games.len(), 2);
        let all_moves: Vec<&str> = r
            .games
            .iter()
            .flat_map(|g| g.moves.iter().map(|(m, _)| m.as_str()))
            .collect();
        assert_eq!(all_moves.len(), 126);
        assert_eq!(all_moves[0], "d4");
        assert_eq!(all_moves[1], "e6");
        assert_eq!(*all_moves.last().unwrap(), "Ke4");
        assert_eq!(all_moves[all_moves.len() - 2], "Rd2+");
    }

    #[test]
    fn test_no_moves_game_termination_multiple_2() {
        let r = parse_file("./tests/pgns/no_moves_but_game_termination_multiple_2.pgn");
        assert_eq!(r.games.len(), 3);

        let all_headers: Vec<String> = r
            .games
            .iter()
            .flat_map(|g| g.headers.iter().map(|(k, v)| format!("{} {}", k, v)))
            .collect();
        assert_eq!(all_headers.len(), 6);

        assert_eq!(
            all_headers[0],
            "FEN 5k2/3r1p2/1p3pp1/p2n3p/P6P/1PPR1PP1/3KN3/6b1 w - - 0 34"
        );
        assert_eq!(all_headers[1], "Result 1/2-1/2");
        assert_eq!(
            all_headers[2],
            "FEN 5k2/5p2/4B2p/r5pn/4P3/5PPP/2NR2K1/8 b - - 0 59"
        );
        assert_eq!(all_headers[3], "Result 1/2-1/2");
        assert_eq!(
            all_headers[4],
            "FEN 8/p3kp1p/1p4p1/2r2b2/2BR3P/1P3P2/P4PK1/8 b - - 0 28"
        );
        assert_eq!(all_headers[5], "Result 1/2-1/2");
    }

    #[test]
    fn test_no_moves_but_comment_followed_by_termination() {
        let r = parse_file("./tests/pgns/no_moves_but_comment_followed_by_termination_marker.pgn");
        assert_eq!(r.games.len(), 2);
        let total: usize = r.games.iter().map(|g| g.moves.len()).sum();
        assert_eq!(total, 2);
    }

    #[test]
    fn test_no_moves_two_games() {
        let r = parse_file("./tests/pgns/no_moves_two_games.pgn");
        assert_eq!(r.games.len(), 2);
        assert!(r.games.iter().all(|g| g.moves.is_empty()));
    }

    #[test]
    fn test_square_bracket_in_header() {
        let r = parse_file("./tests/pgns/square_bracket_in_header.pgn");
        let all_headers: Vec<String> = r
            .games
            .iter()
            .flat_map(|g| g.headers.iter().map(|(k, v)| format!("{} {}", k, v)))
            .collect();
        assert_eq!(
            all_headers[0],
            "Event Batch 10: s20red4c4_t3 vs master[!important]"
        );
        assert_eq!(all_headers[1], "Variation closing ] opening");
        assert_eq!(all_headers[5], "White New-cfe8\"dsadsa\"ce842c");
    }

    #[test]
    fn test_empty_body() {
        let r = parse_file("./tests/pgns/empty_body.pgn");
        assert_eq!(r.games.len(), 2);
        let all_headers: Vec<String> = r
            .games
            .iter()
            .flat_map(|g| g.headers.iter().map(|(k, v)| format!("{} {}", k, v)))
            .collect();
        assert_eq!(all_headers[0], "Event Batch 2690: probTTsv1 vs master");
        assert_eq!(all_headers[7], "Event Batch 269: probTTsv1 vs master");
    }

    #[test]
    fn test_backslash_in_header_returns_error() {
        use std::io::Cursor;
        let data = std::fs::read("./tests/pgns/backslash_header.pgn").expect("failed to read file");
        let mut cursor = Cursor::new(data);
        let mut vis = Recorder::default();
        let err = StreamParser::new(&mut cursor).read_games(&mut vis);
        assert_eq!(
            err,
            Err(StreamParserError(
                StreamParserErrorCode::InvalidHeaderMissingClosingQuote
            ))
        );
        assert_eq!(vis.games.len(), 1);
    }

    /// Parser must not hang on a comment before the first move.
    #[test]
    fn test_comment_before_moves_no_hang() {
        parse_file("./tests/pgns/comment_before_moves.pgn");
    }

    #[test]
    fn test_non_ascii_headers() {
        let r = parse_file("./tests/pgns/non_ascii.pgn");
        assert_eq!(r.games.len(), 1);
        let headers: Vec<String> = r.games[0]
            .headers
            .iter()
            .map(|(k, v)| format!("{} {}", k, v))
            .collect();
        assert_eq!(headers.len(), 7);
        assert_eq!(headers[0], "Event Tournoi François");
        assert_eq!(headers[1], "Site Café");
        assert_eq!(headers[2], "Date 2024.03.15");
        assert_eq!(headers[3], "Round 1");
        assert_eq!(headers[4], "White José María");
        assert_eq!(headers[5], "Black Владимир Петров");
        assert_eq!(headers[6], "Result 1-0");
    }

    #[test]
    fn test_unescaped_quote_header_error() {
        use std::io::Cursor;
        let data =
            std::fs::read("./tests/pgns/unescaped_quote_header.pgn").expect("failed to read file");
        let mut cursor = Cursor::new(data);
        let mut vis = Recorder::default();
        let err = StreamParser::new(&mut cursor).read_games(&mut vis);
        assert_eq!(
            err,
            Err(StreamParserError(
                StreamParserErrorCode::InvalidHeaderMissingClosingBracket
            ))
        );
        assert_eq!(vis.games.len(), 1);
        assert!(vis.games[0].headers.is_empty());
    }

    #[test]
    fn test_empty_input_not_enough_data() {
        use std::io::Cursor;
        let data: Vec<u8> = vec![];
        let mut cursor = Cursor::new(data);
        let mut vis = Recorder::default();
        let err = StreamParser::new(&mut cursor).read_games(&mut vis);
        assert_eq!(
            err,
            Err(StreamParserError(StreamParserErrorCode::NotEnoughData))
        );
    }

    #[test]
    fn test_exceeded_max_string_length_key() {
        use std::io::Cursor;
        // Create a header key longer than STRING_BUFFER_SIZE (255)
        let long_key = "A".repeat(260);
        let pgn = format!("[{} \"v\"]\n\n*\n", long_key);
        let mut cursor = Cursor::new(pgn.into_bytes());
        let mut vis = Recorder::default();
        let err = StreamParser::new(&mut cursor).read_games(&mut vis);
        assert_eq!(
            err,
            Err(StreamParserError(
                StreamParserErrorCode::ExceededMaxStringLength
            ))
        );
    }

    #[test]
    fn test_exceeded_max_string_length_value() {
        use std::io::Cursor;
        // Create a header value longer than STRING_BUFFER_SIZE (255)
        let long_val = "B".repeat(260);
        let pgn = format!("[Event \"{}\"]\n\n*\n", long_val);
        let mut cursor = Cursor::new(pgn.into_bytes());
        let mut vis = Recorder::default();
        let err = StreamParser::new(&mut cursor).read_games(&mut vis);
        assert_eq!(
            err,
            Err(StreamParserError(
                StreamParserErrorCode::ExceededMaxStringLength
            ))
        );
    }

    #[test]
    fn test_error_message_and_none_has_error() {
        let none = StreamParserError::none();
        assert!(!none.has_error());
        assert_eq!(none.message(), "No error");

        let e = StreamParserError(StreamParserErrorCode::ExceededMaxStringLength);
        assert!(e.has_error());
        assert_eq!(e.message(), "Exceeded max string length");
    }

    #[test]
    fn test_default_skip_is_false() {
        struct MinimalVis;
        impl Visitor for MinimalVis {
            fn start_pgn(&mut self) {}
            fn header(&mut self, _: &str, _: &str) {}
            fn start_moves(&mut self) {}
            fn move_token(&mut self, _: &str, _: &str) {}
            fn end_pgn(&mut self) {}
        }

        let v = MinimalVis;
        // default skip should be false
        assert!(!v.skip());
    }

    #[test]
    fn test_no_result_game() {
        let r = parse_file("./tests/pgns/no_result.pgn");
        assert_eq!(r.games.len(), 1);
        let headers: Vec<String> = r.games[0]
            .headers
            .iter()
            .map(|(k, v)| format!("{} {}", k, v))
            .collect();
        assert_eq!(headers.len(), 15);
        assert_eq!(
            headers[7],
            "FEN r1bqk2r/pp1p1pbp/2n2np1/4p3/4P3/2NP2P1/PP2NPBP/R1BQ1RK1 w kq - 0 9"
        );
        assert_eq!(r.games[0].moves[0].0, "");
        assert_eq!(r.games[0].moves[0].1, "No result");
    }
}
