use std::io::Write;

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::queue;
use crossterm::style;
use crossterm::terminal;
use posh_core::engine::parser::lexer::lex;
use posh_core::engine::parser::Token;
use posh_core::path::home_dir;
use posh_core::{Engine, Result};

use super::RawMode;
use crate::config::{Colors, ABBREVIATIONS};

struct State {
    /// The current content of the input line.
    line: String,

    /// The current position the user is on the line.
    index: usize,

    /// The initial position of the terminal grid (start of the line, visually).
    start_pos: (u16, u16),

    /// The size of the terminal window.
    size: (u16, u16),

    /// Will be `true` when the user inputs Enter, ^C, etc.
    about_to_exit: bool,

    /// Will be `true` if the user has just entered ^C.
    cancelled: bool,

    /// Will be `true` if the user just cleared the screen via ^L.
    cleared: bool,

    /// Will be `false` if the user inputs '^ ', which will make abbreviations not expand.
    highlight_abbreviations: bool,
}

impl State {
    fn pos(&self) -> Result<(u16, u16)> {
        Ok(cursor::position()?)
    }
}

pub fn read_line<W: Write>(engine: &mut Engine<W>) -> Result<String> {
    let _raw = RawMode::init()?;

    let mut state = State {
        line: String::new(),
        index: 0,
        start_pos: cursor::position()?,
        size: terminal::size()?,
        about_to_exit: false,
        cancelled: false,
        cleared: false,
        highlight_abbreviations: true,
    };

    while !state.about_to_exit {
        execute!(engine.writer, event::EnableBracketedPaste)?;

        let event = event::read()?;

        if let Event::Paste(s) = &event {
            let (x, y) = state.pos()?;

            state.line.insert_str(state.index, s);
            state.index += s.len();

            execute!(
                engine.writer,
                style::Print(&state.line[state.index - 1..]),
                cursor::MoveTo(x + s.len() as u16, y),
            )?;

            print(engine, &state)?;
        }

        execute!(engine.writer, event::DisableBracketedPaste)?;

        let (code, modifiers) = match event {
            Event::Key(KeyEvent {
                code, modifiers, ..
            }) => (code, modifiers),
            _ => continue,
        };

        match (code, modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                if state.line.is_empty() {
                    continue;
                }

                state.about_to_exit = true;
                state.cancelled = true;
            }

            (KeyCode::Enter, _) => {
                if let Some((expanded_line, _)) = expand_abbreviation(&state.line, true) {
                    state.line = expanded_line;
                }
                state.about_to_exit = true;
            }

            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                if !state.line.is_empty() {
                    continue;
                }

                execute!(engine.writer, style::Print("\n\r"))?;

                // FIXME: better control flow than this
                std::process::exit(0);
            }

            (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                state.line = engine.history.prev()?.cloned().unwrap_or_default();
                state.index = state.line.len();

                let (start_x, start_y) = state.start_pos;
                execute!(
                    engine.writer,
                    cursor::MoveTo(start_x + state.index as u16, start_y)
                )?;
            }

            (KeyCode::Down, _) | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                state.line = engine.history.next()?.cloned().unwrap_or_default();
                state.index = state.line.len();

                let (start_x, start_y) = state.start_pos;
                execute!(
                    engine.writer,
                    cursor::MoveTo(start_x + state.index as u16, start_y)
                )?;
            }

            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                state.line.clear();
                state.index = 0;
                let (start_x, start_y) = state.start_pos;
                execute!(engine.writer, cursor::MoveTo(start_x, start_y))?;
            }

            (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
                if state.index == 0 {
                    continue;
                }

                let mut space_index = None;
                for i in (0..state.index).rev() {
                    if let Some(' ') = state.line.chars().nth(i) {
                        space_index = Some(i);
                        break;
                    }
                }

                if let Some(' ') = state.line.chars().nth(state.index - 1) {
                    // FIXME: this should find the previous space
                    space_index = Some(0);
                }

                let space_index = space_index.unwrap_or(0);
                let offset = (state.index - space_index) as u16;
                state.line.replace_range(space_index..state.index, "");
                state.index = space_index;

                if has_abbreviation(&state.line) {
                    state.highlight_abbreviations = true;
                }

                execute!(engine.writer, cursor::MoveLeft(offset))?;
            }

            (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                execute!(
                    engine.writer,
                    terminal::Clear(terminal::ClearType::All),
                    cursor::MoveTo(0, 0)
                )?;
                state.cleared = true;
                break;
            }

            (KeyCode::Left, _) | (KeyCode::Char('b'), KeyModifiers::CONTROL) if state.index > 0 => {
                state.index -= 1;
                execute!(engine.writer, cursor::MoveLeft(1))?;
            }

            (KeyCode::Right, _) | (KeyCode::Char('f'), KeyModifiers::CONTROL)
                if state.index < state.line.len() =>
            {
                state.index += 1;
                execute!(engine.writer, cursor::MoveRight(1))?;
            }

            (KeyCode::Char(c @ (' ' | '|' | ';')), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                let (mut x, y) = state.pos()?;

                if state.line.find(' ').is_none() {
                    if let Some((expanded_line, diff)) = expand_abbreviation(&state.line, false) {
                        state.line = expanded_line;

                        // FIXME: replace with something like `wrapping_add_signed` once
                        //        https://github.com/rust-lang/rust/issues/87840 is in stable
                        if diff >= 0 {
                            x = u16::checked_add(x, diff as u16).unwrap_or(0);
                            state.index =
                                usize::checked_add(state.index, diff as usize).unwrap_or(0);
                        } else {
                            x = u16::checked_sub(x, diff.unsigned_abs() as u16).unwrap_or(0);
                            state.index =
                                usize::checked_sub(state.index, diff.unsigned_abs()).unwrap_or(0);
                        }
                    }
                }

                state.line.insert(state.index, c);
                state.index += 1;

                if has_abbreviation(&state.line) {
                    state.highlight_abbreviations = true;
                }

                execute!(
                    engine.writer,
                    terminal::Clear(terminal::ClearType::UntilNewLine),
                    style::Print(&state.line[state.index - 1..]),
                    cursor::MoveTo(x + 1, y),
                )?;
            }

            (KeyCode::Char(' '), KeyModifiers::CONTROL) => {
                let (x, y) = state.pos()?;

                state.line.insert(state.index, ' ');
                state.index += 1;

                state.highlight_abbreviations = false;

                execute!(
                    engine.writer,
                    terminal::Clear(terminal::ClearType::UntilNewLine),
                    style::Print(&state.line[state.index - 1..]),
                    cursor::MoveTo(x + 1, y),
                )?;
            }

            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                let (x, y) = state.pos()?;

                state.line.insert(state.index, c);
                state.index += 1;

                execute!(
                    engine.writer,
                    style::Print(&state.line[state.index - 1..]),
                    cursor::MoveTo(x + 1, y),
                )?;
            }

            (KeyCode::Backspace, _) if state.index > 0 => {
                let (x, y) = state.pos()?;

                state.index -= 1;
                state.line.remove(state.index);

                if has_abbreviation(&state.line) {
                    state.highlight_abbreviations = true;
                }

                execute!(
                    engine.writer,
                    cursor::MoveTo(x - 1, y),
                    style::Print(&state.line[state.index..]),
                    cursor::MoveTo(x - 1, y),
                )?;
            }

            _ => {}
        }

        print(engine, &state)?;

        if state.about_to_exit {
            break;
        }
    }

    write!(engine.writer, "\r")?;
    let (_, start_y) = state.start_pos;
    let (_, height) = state.size;
    if start_y + 1 >= height {
        execute!(engine.writer, terminal::ScrollUp(1))?;
    }

    if !state.cleared {
        execute!(engine.writer, cursor::MoveTo(0, start_y + 1))?;
    } else {
        execute!(engine.writer, cursor::MoveTo(0, 0))?;
    }

    // FIXME: should probably return Result<Option<String>> with Ok(None) here?
    if state.cancelled {
        Ok("".to_string())
    } else {
        Ok(state.line)
    }
}

fn should_highlight_command(prev_token: Option<&Token>) -> bool {
    if prev_token.is_none() {
        return true;
    }

    if let Some(Token::Pipe | Token::Semicolon | Token::RedirectOutput(_, _, _, _)) = prev_token {
        return true;
    }

    if let Some(token @ Token::String(_)) = prev_token {
        if token.try_get_assignment().is_some() {
            return true;
        }
    }

    false
}

fn should_highlight_assignment(prev_token: Option<&Token>) -> bool {
    let mut should_highlight_assignment = matches!(
        prev_token,
        Some(Token::Pipe | Token::Semicolon | Token::RedirectOutput(_, _, _, _)) | None
    );

    if let Some(token @ Token::String(_)) = prev_token {
        if token.try_get_assignment().is_some() {
            should_highlight_assignment = true;
        }
    }

    should_highlight_assignment
}

pub fn has_abbreviation(cmd: impl AsRef<str>) -> bool {
    let cmd = cmd.as_ref();
    ABBREVIATIONS.iter().any(|&(a, _)| a == cmd)
}

// FIXME: highlighting does not work in command substitutions,
//        since we are not aware of them because we're using
//        tokens to highlight instead of the AST
fn print<W: Write>(engine: &mut Engine<W>, state: &State) -> Result<()> {
    // Perhaps it would be preferable to use the AST to highlight.
    // Using the tokens is kind of hacky (e.g. since it highlights
    // commands based on what the previous token was)
    let tokens = lex(&state.line, true);

    let mut prev_non_space_token = None;

    let (start_x, start_y) = state.start_pos;
    let (x, y) = state.pos()?;

    queue!(
        engine.writer,
        cursor::MoveTo(start_x, start_y),
        terminal::Clear(terminal::ClearType::UntilNewLine)
    )?;

    for token in &tokens {
        match token {
            Token::Space => queue!(engine.writer, style::Print(" "))?,

            str_token @ (Token::String(s)
            | Token::SingleQuotedString(s, _)
            | Token::DoubleQuotedString(s, _)) => {
                match prev_non_space_token {
                    // If this is the first token, or if it's directly after any
                    // command separator, it should be highlighted as a command.
                    token if should_highlight_command(token) => {
                        let color = if engine.has_builtin(s) {
                            Colors::VALID_BUILTIN
                        } else if engine.has_command(s) {
                            Colors::VALID_CMD
                        } else if has_abbreviation(s) && state.highlight_abbreviations {
                            Colors::VALID_ABBR
                        } else if s.starts_with("~/") {
                            // FIXME: This block is a hack to make syntax highlighting work.
                            //        The has_command() function expects expanded lines, but
                            //        since we don't have a CommandType available here, we
                            //        have to mock the "expansion".
                            let expanded_s = s.replacen('~', &home_dir(), 1);
                            if engine.has_command(expanded_s) {
                                Colors::VALID_CMD
                            } else {
                                Colors::INVALID_CMD
                            }
                        } else {
                            Colors::INVALID_CMD
                        };
                        queue!(engine.writer, style::SetForegroundColor(color))?;
                    }

                    _ => match str_token {
                        Token::String(s) => queue!(
                            engine.writer,
                            style::SetForegroundColor(if s.starts_with('-') {
                                Colors::FLAG
                            } else {
                                Colors::STRING
                            })
                        )?,

                        Token::SingleQuotedString(_, finished) => queue!(
                            engine.writer,
                            style::SetForegroundColor(if *finished {
                                Colors::SINGLE_QUOTED_STRING
                            } else {
                                Colors::INCOMPLETE
                            })
                        )?,

                        Token::DoubleQuotedString(_, finished) => queue!(
                            engine.writer,
                            style::SetForegroundColor(if *finished {
                                Colors::DOUBLE_QUOTED_STRING
                            } else {
                                Colors::INCOMPLETE
                            })
                        )?,

                        _ => unreachable!(),
                    },
                }

                match str_token {
                    token @ Token::String(s) => {
                        let highlight_assignment =
                            should_highlight_assignment(prev_non_space_token);

                        match token.try_get_assignment() {
                            Some((a, None)) if highlight_assignment => {
                                queue!(
                                    engine.writer,
                                    style::SetForegroundColor(Colors::STRING),
                                    style::Print(a),
                                    style::SetForegroundColor(Colors::ASSIGNMENT),
                                    style::Print('=')
                                )?;
                            }
                            Some((a, Some(b))) if highlight_assignment => {
                                queue!(
                                    engine.writer,
                                    style::SetForegroundColor(Colors::STRING),
                                    style::Print(a),
                                    style::SetForegroundColor(Colors::ASSIGNMENT),
                                    style::Print('='),
                                    style::SetForegroundColor(Colors::STRING),
                                    style::Print(b),
                                )?;
                            }
                            _ => queue!(engine.writer, style::Print(s))?,
                        }
                    }

                    Token::SingleQuotedString(_, true) => {
                        queue!(engine.writer, style::Print(format!("'{s}'")))?
                    }

                    Token::DoubleQuotedString(_, true) => {
                        queue!(engine.writer, style::Print(format!("\"{s}\"")))?
                    }

                    Token::SingleQuotedString(_, false) => {
                        queue!(engine.writer, style::Print(format!("'{s}")))?
                    }

                    Token::DoubleQuotedString(_, false) => {
                        queue!(engine.writer, style::Print(format!("\"{s}")))?
                    }

                    _ => unreachable!(),
                }
            }

            Token::Pipe => queue!(
                engine.writer,
                style::SetForegroundColor(Colors::PIPE),
                style::Print("|")
            )?,

            Token::Semicolon => queue!(
                engine.writer,
                style::SetForegroundColor(Colors::SEMICOLON),
                style::Print(";")
            )?,

            Token::RedirectOutput(None, to, Some(space), false) => queue!(
                engine.writer,
                style::SetForegroundColor(Colors::REDIRECT_OUTPUT),
                style::Print(format!(">{space}{to}"))
            )?,

            Token::RedirectOutput(None, to, Some(space), true) => queue!(
                engine.writer,
                style::SetForegroundColor(Colors::REDIRECT_OUTPUT),
                style::Print(format!(">>{space}{to}"))
            )?,

            Token::RedirectOutput(None, to, None, false) => queue!(
                engine.writer,
                style::SetForegroundColor(Colors::REDIRECT_OUTPUT),
                style::Print(format!(">{to}"))
            )?,

            Token::RedirectOutput(None, to, None, true) => queue!(
                engine.writer,
                style::SetForegroundColor(Colors::REDIRECT_OUTPUT),
                style::Print(format!(">>{to}"))
            )?,

            Token::RedirectOutput(Some(from), to, None, false) => queue!(
                engine.writer,
                style::SetForegroundColor(Colors::REDIRECT_OUTPUT),
                style::Print(format!("{from}>{to}"))
            )?,

            Token::RedirectOutput(Some(from), to, None, true) => queue!(
                engine.writer,
                style::SetForegroundColor(Colors::REDIRECT_OUTPUT),
                style::Print(format!("{from}>>{to}"))
            )?,

            Token::RedirectOutput(Some(from), to, Some(space), false) => queue!(
                engine.writer,
                style::SetForegroundColor(Colors::REDIRECT_OUTPUT),
                style::Print(format!("{from}>{space}{to}"))
            )?,

            Token::RedirectOutput(Some(from), to, Some(space), true) => queue!(
                engine.writer,
                style::SetForegroundColor(Colors::REDIRECT_OUTPUT),
                style::Print(format!("{from}>>{space}{to}"))
            )?,

            Token::RedirectInput(to) => queue!(
                engine.writer,
                style::SetForegroundColor(Colors::REDIRECT_INPUT),
                style::Print(format!("<{to}"))
            )?,

            Token::And => queue!(
                engine.writer,
                style::SetForegroundColor(Colors::NYI),
                style::Print("&&")
            )?,

            Token::Or => queue!(
                engine.writer,
                style::SetForegroundColor(Colors::NYI),
                style::Print("||")
            )?,

            Token::Ampersand => queue!(
                engine.writer,
                style::SetForegroundColor(Colors::NYI),
                style::Print("&")
            )?,
        }

        if !matches!(&token, &Token::Space) {
            prev_non_space_token = Some(token);
        }
    }

    if state.cancelled {
        queue!(engine.writer, style::ResetColor, style::Print("^C"))?;
    }

    queue!(engine.writer, style::ResetColor, cursor::MoveTo(x, y))?;

    engine.writer.flush()?;

    Ok(())
}

fn expand_abbreviation<S: AsRef<str>>(line: S, only_if_equal: bool) -> Option<(String, isize)> {
    let line = line.as_ref();
    for (a, b) in ABBREVIATIONS {
        if line == a || (!only_if_equal && line.starts_with(&format!("{a} "))) {
            let diff = b.len() as isize - a.len() as isize;
            return Some((line.replacen(a, b, 1), diff));
        }
    }
    None
}
