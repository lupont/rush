use std::ops::RangeInclusive;

use crate::path::home_dir;
use crate::{Error, Result};

use super::{util, Token};

pub fn parse(line: impl AsRef<str>) -> SyntaxTree {
    let tokens = super::lex(line, false);
    parse_tokens(tokens)
}

pub trait Expand: Sized {
    fn expand(self) -> Result<Self>;
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SyntaxTree {
    pub commands: Vec<CommandType>,
}

impl SyntaxTree {
    pub fn new() -> Self {
        Self {
            commands: Default::default(),
        }
    }

    pub fn add_command(&mut self, command: CommandType) {
        self.commands.push(command);
    }
}

impl Default for SyntaxTree {
    fn default() -> Self {
        Self::new()
    }
}

impl ToString for SyntaxTree {
    fn to_string(&self) -> String {
        self.commands
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum CommandType {
    Single(Command),
    Pipeline(Vec<Command>),
}

impl Expand for CommandType {
    fn expand(self) -> Result<Self> {
        match self {
            Self::Single(cmd) => Ok(Self::Single(cmd.expand()?)),
            Self::Pipeline(cmds) => Ok(Self::Pipeline(
                cmds.into_iter()
                    .map(|c| c.expand())
                    .collect::<Result<Vec<_>>>()?,
            )),
        }
    }
}

impl ToString for CommandType {
    fn to_string(&self) -> String {
        match self {
            Self::Single(cmd) => cmd.to_string(),
            Self::Pipeline(cmds) => cmds
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" | "),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Command {
    pub name: Word,
    pub prefixes: Vec<Meta>,
    pub suffixes: Vec<Meta>,
}

fn expand_meta(vars: &[(String, String)], meta: Meta) -> Result<Meta> {
    match meta {
        Meta::Redirect(redirect) => match redirect {
            Redirect::Output {
                from: Some(from),
                to,
                append,
            } => Ok(Meta::Redirect(Redirect::Output {
                from: Some(expand_word(vars, from)?),
                to: expand_word(vars, to)?,
                append,
            })),
            Redirect::Output {
                from: None,
                to,
                append,
            } => Ok(Meta::Redirect(Redirect::Output {
                from: None,
                to: expand_word(vars, to)?,
                append,
            })),
            Redirect::Input { to } => Ok(Meta::Redirect(Redirect::Input {
                to: expand_word(vars, to)?,
            })),
        },
        Meta::Word(word) => Ok(Meta::Word(expand_word(vars, word)?)),
        Meta::Assignment(var, val) => Ok(Meta::Assignment(
            expand_word(vars, var)?,
            expand_word(vars, val)?,
        )),
    }
}

fn expand_word(vars: &[(String, String)], mut word: Word) -> Result<Word> {
    if word.expansions.is_empty() {
        return Ok(word);
    }

    let mut to_remove = Vec::new();

    for (i, expansion) in word.expansions.iter().enumerate().rev() {
        match expansion {
            Expansion::Tilde { index } => {
                let home = home_dir();
                word.name.replace_range(index..=index, &home);
                to_remove.push(i);
            }

            Expansion::Parameter { range, name } => {
                for (var, val) in vars.iter() {
                    if var == name {
                        word.name.replace_range(range.clone(), val);
                        to_remove.push(i);
                    }
                }
            }

            Expansion::Command { range: _, ast: _ } => {
                return Err(Error::Unimplemented(
                    "command expansions are not yet implemented".to_string(),
                ))
            }

            Expansion::Glob {
                range: _,
                pattern: _,
                recursive: _,
            } => {
                return Err(Error::Unimplemented(
                    "glob expansions are not yet implemented".to_string(),
                ))
            }
        }
    }

    to_remove.sort();
    to_remove.dedup();
    for i in to_remove.into_iter().rev() {
        word.expansions.remove(i);
    }

    Ok(word)
}

impl Expand for Command {
    fn expand(mut self) -> Result<Self> {
        let vars = self.vars();
        self.name = expand_word(&vars, self.name)?;

        self.prefixes = self
            .prefixes
            .into_iter()
            .map(|m| expand_meta(&vars, m))
            .collect::<Result<Vec<_>>>()?;

        self.suffixes = self
            .suffixes
            .into_iter()
            .map(|m| expand_meta(&vars, m))
            .collect::<Result<Vec<_>>>()?;

        Ok(self)
    }
}

impl Command {
    pub fn cmd_name(&self) -> &String {
        &self.name.name
    }

    pub fn vars(&self) -> Vec<(String, String)> {
        let mut vars = Vec::new();

        for (var, val) in std::env::vars() {
            vars.push((var, val));
        }

        for meta in self.prefixes.clone().into_iter().filter_map(|m| {
            if let Meta::Assignment(_, _) = m {
                Some(m)
            } else {
                None
            }
        }) {
            if let Ok(Meta::Assignment(var, val)) = expand_meta(&vars, meta) {
                if let Some(index) = vars.iter().position(|(v, _)| v == &var.name) {
                    vars.remove(index);
                }
                vars.push((var.name.clone(), val.name.clone()));
            }
        }

        vars
    }

    pub fn redirections(&self) -> (Option<Redirect>, Option<Redirect>, Option<Redirect>) {
        let mut stdin_redirect = None;
        let mut stdout_redirect = None;
        let mut stderr_redirect = None;

        for meta in self
            .prefixes
            .iter()
            .cloned()
            .chain(self.suffixes.iter().cloned())
        {
            if let Meta::Redirect(redirect) = meta {
                match redirect {
                    Redirect::Output { from: None, .. } => stdout_redirect = Some(redirect.clone()),

                    Redirect::Output {
                        from: Some(ref s), ..
                    } if s.name == "1" => stdout_redirect = Some(redirect.clone()),

                    Redirect::Output {
                        from: Some(ref s), ..
                    } if s.name == "2" => stderr_redirect = Some(redirect.clone()),

                    Redirect::Input { .. } => stdin_redirect = Some(redirect.clone()),

                    _ => {}
                }
            }
        }

        (stdin_redirect, stdout_redirect, stderr_redirect)
    }

    pub fn args(&self) -> Vec<String> {
        self.suffixes
            .iter()
            .filter_map(|m| {
                if let Meta::Word(w) = m {
                    Some(w.name.clone())
                } else {
                    None
                }
            })
            .collect()
    }
}

impl ToString for Command {
    fn to_string(&self) -> String {
        let prefixes = self
            .prefixes
            .iter()
            .map(|s| s.to_string().trim().to_string())
            .collect::<Vec<_>>()
            .join(" ");

        let suffixes = self
            .suffixes
            .iter()
            .map(|s| s.to_string().trim().to_string())
            .collect::<Vec<_>>()
            .join(" ");

        format!(
            "{}{}{}",
            if prefixes.is_empty() {
                "".to_string()
            } else {
                prefixes + " "
            },
            self.name.to_string() + if suffixes.is_empty() { "" } else { " " },
            suffixes,
        )
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Word {
    pub name: String,
    pub expansions: Vec<Expansion>,
}

impl Word {
    fn new(name: impl ToString, expansions: Vec<Expansion>) -> Self {
        Self {
            name: name.to_string(),
            expansions,
        }
    }
}

impl ToString for Word {
    fn to_string(&self) -> String {
        self.name.clone()
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Meta {
    Redirect(Redirect),
    Word(Word),
    Assignment(Word, Word),
}

impl ToString for Meta {
    fn to_string(&self) -> String {
        match self {
            Self::Word(word) => word.name.clone(),
            Self::Redirect(redirect) => match redirect {
                Redirect::Input { to } => format!("<{}", to.name),
                Redirect::Output {
                    from: None,
                    to,
                    append: false,
                } => format!(">{}", to.name),
                Redirect::Output {
                    from: None,
                    to,
                    append: true,
                } => format!(">>{}", to.name),
                Redirect::Output {
                    from: Some(from),
                    to,
                    append: false,
                } => format!("{}>{}", from.name, to.name),
                Redirect::Output {
                    from: Some(from),
                    to,
                    append: true,
                } => format!("{}>>{}", from.name, to.name),
            },
            Self::Assignment(var, val) => format!("{}={}", var.name, val.name),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Redirect {
    Output {
        from: Option<Word>,
        to: Word,
        append: bool,
    },
    Input {
        to: Word,
    },
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Expansion {
    Parameter {
        range: RangeInclusive<usize>,
        name: String,
    },

    Command {
        range: RangeInclusive<usize>,
        ast: SyntaxTree,
    },

    Glob {
        range: RangeInclusive<usize>,
        pattern: String,
        recursive: bool,
    },

    Tilde {
        index: usize,
    },
}

fn parse_tokens(tokens: Vec<Token>) -> SyntaxTree {
    // Split tokens by semicolons to get list of commands,
    // then each command by pipe to get pipeline in command
    let commands = tokens
        .split(|t| matches!(t, Token::Semicolon))
        .map(|tokens| {
            tokens
                .split(|t| matches!(t, Token::Pipe))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    let mut ast = SyntaxTree {
        commands: Vec::with_capacity(commands.len()),
    };

    for pipeline in commands {
        match &pipeline[..] {
            &[cmd] if !cmd.is_empty() => {
                if let Some(cmd) = parse_command(cmd) {
                    ast.add_command(CommandType::Single(cmd));
                } else {
                    panic!("could not parse command");
                    // FIXME: syntax error?
                }
            }

            cmds => {
                let mut commands = Vec::new();

                for &command in cmds {
                    if command.is_empty() {
                        continue;
                    }

                    if let Some(cmd) = parse_command(command) {
                        commands.push(cmd);
                    } else {
                        // FIXME: syntax error?
                        panic!("could not parse command");
                    }
                }

                if !commands.is_empty() {
                    ast.add_command(CommandType::Pipeline(commands));
                }
            }
        };
    }

    ast
}

fn parse_command(tokens: &[Token]) -> Option<Command> {
    let tokens = tokens.iter().peekable();

    let mut name = None;
    let mut prefixes = Vec::new();
    let mut suffixes = Vec::new();

    for token in tokens {
        match token {
            token @ (Token::String(_)
            | Token::SingleQuotedString(_, _)
            | Token::DoubleQuotedString(_, _)) => match parse_meta(token, name.is_none()) {
                Some(word @ Meta::Word(_)) => {
                    if name.is_none() {
                        name = Some(word);
                    } else {
                        suffixes.push(word);
                    }
                }
                Some(Meta::Assignment(dest, var)) => {
                    if name.is_none() {
                        prefixes.push(Meta::Assignment(dest, var));
                    } else {
                        suffixes.push(Meta::Assignment(dest, var));
                    }
                }
                Some(meta) => panic!("disallowed type: {:?}", meta),
                None => {}
            },

            token @ Token::RedirectOutput(_, _, _, _) => {
                if let Some(redirect) = parse_meta(token, name.is_none()) {
                    match name {
                        None => prefixes.push(redirect),
                        Some(_) => suffixes.push(redirect),
                    }
                }
            }

            Token::RedirectInput(_) => {
                if let Some(redirect) = parse_meta(token, name.is_none()) {
                    match name {
                        None => prefixes.push(redirect),
                        Some(_) => suffixes.push(redirect),
                    }
                }
            }

            // Token::LParen => todo!("( subshells are not yet implemented"),
            // Token::RParen => todo!(") subshells are not yet implemented"),

            // Token::LBrace => todo!("{{ command grouping is not yet implemented"),
            // Token::RBrace => todo!("}} command grouping is not yet implemented"),
            Token::And => todo!("AND is not yet implemented"),
            Token::Or => todo!("OR is not yet implemented"),

            Token::Space => {}

            Token::Ampersand => todo!("asynchronous execution is not yet implemented"),

            Token::Semicolon => unreachable!("semicolons should have been found already"),
            Token::Pipe => unreachable!("pipes should have been found already"),
        }
    }

    if let Some(Meta::Word(name)) = name {
        Some(Command {
            name,
            prefixes,
            suffixes,
        })
    } else {
        eprintln!("{name:?}");
        None
    }
}

enum ExpansionType {
    All,
    VariablesAndCommands,
    None,
}

fn parse_word(s: impl AsRef<str>, expand: ExpansionType) -> Word {
    if let ExpansionType::None = expand {
        return Word::new(s.as_ref(), Vec::new());
    }

    let s = s.as_ref();
    let mut chars = s.chars().peekable();
    let mut expansions = Vec::new();
    let mut index = 0;

    let mut prev_char = None;

    while let Some(ch) = chars.next() {
        match ch {
            ' ' => {}

            // should be guarded by !matches!(expand, Expand::None), but since
            // we have an early return specifically for Expand::None, it is not
            // needed.
            '$' => match chars.peek() {
                Some(&c) if util::is_valid_first_character_of_expansion(c) => {
                    let c = chars.next().unwrap();

                    let mut var = c.to_string();
                    let start_index = index;

                    while let Some(&c) = chars.peek() {
                        if !util::is_valid_first_character_of_expansion(c) {
                            break;
                        }
                        var.push(chars.next().unwrap());
                        index += 1;
                    }

                    index += 1;

                    expansions.push(Expansion::Parameter {
                        name: var,
                        range: start_index..=index,
                    });
                }

                Some(&'(') => {
                    let mut nested_level = 0;
                    let start_index = index;
                    chars.next();
                    let mut subcmd = String::new();
                    while let Some(next) = chars.next() {
                        if next == '$' {
                            if let Some(&'(') = chars.peek() {
                                nested_level += 1;
                            }
                        }
                        index += 1;
                        if next == ')' {
                            if nested_level > 0 {
                                nested_level -= 1;
                            } else {
                                break;
                            }
                        }
                        subcmd.push(next);
                    }
                    index += 1;
                    let ast = parse(subcmd);
                    expansions.push(Expansion::Command {
                        ast,
                        range: start_index..=index,
                    });
                }

                c => panic!("got unexpected: {c:?}"),
            },

            '*' if matches!(expand, ExpansionType::All) => {
                let mut recursive = false;
                let mut pattern = '*'.to_string();
                let start_index = index;

                while let Some(&c) = chars.peek() {
                    match c {
                        '*' => {
                            chars.next();
                            index += 1;
                            recursive = true;
                            pattern.push('*');
                        }

                        c => {
                            if " /".contains(c) {
                                break;
                            }
                            pattern.push(c);
                            chars.next();
                            index += 1;
                        }
                    }
                }

                expansions.push(Expansion::Glob {
                    pattern,
                    recursive,
                    range: start_index..=index,
                });
            }

            '~' if matches!(expand, ExpansionType::All)
                && matches!(prev_char, Some(' ' | '=') | None) =>
            {
                match chars.peek() {
                    Some(' ') | Some('/') | None => {
                        expansions.push(Expansion::Tilde { index });
                    }
                    _ => {}
                }
            }

            _ => {}
        }
        index += 1;
        prev_char = Some(ch);
    }

    Word::new(s, expansions)
}

fn parse_meta(token: &Token, is_prefix: bool) -> Option<Meta> {
    match token {
        Token::String(s) => {
            if is_prefix {
                let item = match s.split_once('=') {
                    Some((var, val)) => {
                        let var_word = parse_word(var, ExpansionType::None);
                        let val_word = parse_word(val, ExpansionType::All);
                        Meta::Assignment(var_word, val_word)
                    }
                    None => Meta::Word(parse_word(s, ExpansionType::All)),
                };

                Some(item)
            } else {
                Some(Meta::Word(parse_word(s, ExpansionType::All)))
            }
        }

        Token::SingleQuotedString(s, finished) => {
            if *finished {
                let word = parse_word(s, ExpansionType::None);
                Some(Meta::Word(word))
            } else {
                // FIXME: syntax error
                None
            }
        }

        Token::DoubleQuotedString(s, finished) => {
            if *finished {
                let word = parse_word(s, ExpansionType::VariablesAndCommands);
                Some(Meta::Word(word))
            } else {
                // FIXME: syntax error
                None
            }
        }

        // FIXME: this should probably not always use ExpansionType::All
        Token::RedirectInput(s) => Some(Meta::Redirect(Redirect::Input {
            to: parse_word(s, ExpansionType::All),
        })),

        Token::RedirectOutput(from, to, _, append) => {
            let from = match from {
                Some(s) => Some(s),
                None => None,
            };
            let to = to.to_string();
            // FIXME: these should probably not always use ExpansionType::All
            Some(Meta::Redirect(Redirect::Output {
                from: from.cloned().map(|s| parse_word(s, ExpansionType::All)),
                to: parse_word(to, ExpansionType::All),
                append: *append,
            }))
        }

        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_parsing() {
        let input = "2>&1 echo hello world | lolcat -n;".to_string();
        let ast = parse(input);

        println!("{:#?}", &ast);

        assert_eq!(
            SyntaxTree {
                commands: vec![CommandType::Pipeline(vec![
                    Command {
                        name: Word::new("echo", vec![]),
                        prefixes: vec![Meta::Redirect(Redirect::Output {
                            from: Some(Word::new("2", vec![])),
                            to: Word::new("&1", vec![]),
                            append: false,
                        }),],
                        suffixes: vec![
                            Meta::Word(Word::new("hello", vec![])),
                            Meta::Word(Word::new("world", vec![])),
                        ],
                    },
                    Command {
                        name: Word::new("lolcat", vec![]),
                        prefixes: vec![],
                        suffixes: vec![Meta::Word(Word::new("-n", vec![])),],
                    }
                ]),],
            },
            ast
        );
    }

    #[test]
    fn asterisk_expansion_parsing() {
        let input = "echo **/*.rs".to_string();
        let ast = parse(input);

        let expected = SyntaxTree {
            commands: vec![CommandType::Single(Command {
                name: Word::new("echo", vec![]),
                prefixes: vec![],
                suffixes: vec![Meta::Word(Word::new(
                    "**/*.rs",
                    vec![
                        Expansion::Glob {
                            pattern: "**".into(),
                            recursive: true,
                            range: 0..=1,
                        },
                        Expansion::Glob {
                            pattern: "*.rs".into(),
                            recursive: false,
                            range: 3..=6,
                        },
                    ],
                ))],
            })],
        };
        assert_eq!(expected, ast);
    }

    #[test]
    fn variable_expansion_parsing() {
        let input = "echo \"yo $foo $A\"".to_string();
        let ast = parse(input);

        assert_eq!(
            SyntaxTree {
                commands: vec![CommandType::Single(Command {
                    name: Word::new("echo", vec![]),
                    prefixes: vec![],
                    suffixes: vec![Meta::Word(Word::new(
                        "yo $foo $A",
                        vec![
                            Expansion::Parameter {
                                name: "foo".into(),
                                range: 3..=6,
                            },
                            Expansion::Parameter {
                                name: "A".into(),
                                range: 8..=9,
                            },
                        ],
                    )),],
                })],
            },
            ast
        );
    }

    #[test]
    fn single_quote_doesnt_expand_parsing() {
        let input = "echo '** $foo'".to_string();
        let ast = parse(input);

        let expected = SyntaxTree {
            commands: vec![CommandType::Single(Command {
                name: Word::new("echo", vec![]),
                prefixes: vec![],
                suffixes: vec![Meta::Word(Word::new("** $foo", vec![]))],
            })],
        };

        assert_eq!(expected, ast);
    }

    #[test]
    fn nested_pipeline_parsing() {
        let input = r#"echo "I \"am\": $(whoami | rev | grep -o -v foo)" | less"#.to_string();
        let ast = parse(input);

        let expected = SyntaxTree {
            commands: vec![CommandType::Pipeline(vec![
                Command {
                    name: Word::new("echo", vec![]),
                    prefixes: vec![],
                    suffixes: vec![Meta::Word(Word::new(
                        "I \"am\": $(whoami | rev | grep -o -v foo)",
                        vec![Expansion::Command {
                            range: 8..=39,
                            ast: SyntaxTree {
                                commands: vec![CommandType::Pipeline(vec![
                                    Command {
                                        name: Word::new("whoami", vec![]),
                                        prefixes: vec![],
                                        suffixes: vec![],
                                    },
                                    Command {
                                        name: Word::new("rev", vec![]),
                                        prefixes: vec![],
                                        suffixes: vec![],
                                    },
                                    Command {
                                        name: Word::new("grep", vec![]),
                                        prefixes: vec![],
                                        suffixes: vec![
                                            Meta::Word(Word::new("-o", vec![])),
                                            Meta::Word(Word::new("-v", vec![])),
                                            Meta::Word(Word::new("foo", vec![])),
                                        ],
                                    },
                                ])],
                            },
                        }],
                    ))],
                },
                Command {
                    name: Word::new("less", vec![]),
                    prefixes: vec![],
                    suffixes: vec![],
                },
            ])],
        };

        assert_eq!(expected, ast);
    }

    #[test]
    fn complicated_parsing() {
        let input = r#"CMD=exec=async 2>&1 grep ": $(whoami)" ~/.cache/ | xargs -I {} echo "$CMD: {}" >foo.log"#.to_string();
        let ast = parse(input);

        let expected = SyntaxTree {
            commands: vec![CommandType::Pipeline(vec![
                Command {
                    name: Word::new("grep", vec![]),
                    prefixes: vec![
                        Meta::Assignment(Word::new("CMD", vec![]), Word::new("exec=async", vec![])),
                        Meta::Redirect(Redirect::Output {
                            from: Some(Word::new("2", vec![])),
                            to: Word::new("&1", vec![]),
                            append: false,
                        }),
                    ],
                    suffixes: vec![
                        Meta::Word(Word::new(
                            ": $(whoami)",
                            vec![Expansion::Command {
                                range: 2..=10,
                                ast: SyntaxTree {
                                    commands: vec![CommandType::Single(Command {
                                        name: Word::new("whoami", vec![]),
                                        prefixes: vec![],
                                        suffixes: vec![],
                                    })],
                                },
                            }],
                        )),
                        Meta::Word(Word::new("~/.cache/", vec![Expansion::Tilde { index: 0 }])),
                    ],
                },
                Command {
                    name: Word::new("xargs", vec![]),
                    prefixes: vec![],
                    suffixes: vec![
                        Meta::Word(Word::new("-I", vec![])),
                        Meta::Word(Word::new("{}", vec![])),
                        Meta::Word(Word::new("echo", vec![])),
                        Meta::Word(Word::new(
                            "$CMD: {}",
                            vec![Expansion::Parameter {
                                name: "CMD".into(),
                                range: 0..=3,
                            }],
                        )),
                        Meta::Redirect(Redirect::Output {
                            from: None,
                            to: Word::new("foo.log", vec![]),
                            append: false,
                        }),
                    ],
                },
            ])],
        };

        assert_eq!(expected, ast);
    }

    #[test]
    fn basic_command_expansion_parsing() {
        let input = r#"echo "bat: $(cat /sys/class/power_supply/BAT0/capacity)""#.to_string();
        let ast = parse(input);

        let expected = SyntaxTree {
            commands: vec![CommandType::Single(Command {
                name: Word::new("echo", vec![]),
                prefixes: vec![],
                suffixes: vec![Meta::Word(Word::new(
                    "bat: $(cat /sys/class/power_supply/BAT0/capacity)",
                    vec![Expansion::Command {
                        range: 5..=48,
                        ast: SyntaxTree {
                            commands: vec![CommandType::Single(Command {
                                name: Word::new("cat", vec![]),
                                prefixes: vec![],
                                suffixes: vec![Meta::Word(Word::new(
                                    "/sys/class/power_supply/BAT0/capacity",
                                    vec![],
                                ))],
                            })],
                        },
                    }],
                ))],
            })],
        };

        assert_eq!(expected, ast);
    }

    #[test]
    fn tilde_expansion_parsing() {
        let input = "ls ~ ~/ ~/foo foo~ bar/~ ./~ ~% ~baz".to_string();
        let ast = parse(input);

        let expected = SyntaxTree {
            commands: vec![CommandType::Single(Command {
                name: Word::new("ls", vec![]),
                prefixes: vec![],
                suffixes: vec![
                    Meta::Word(Word::new("~", vec![Expansion::Tilde { index: 0 }])),
                    Meta::Word(Word::new("~/", vec![Expansion::Tilde { index: 0 }])),
                    Meta::Word(Word::new("~/foo", vec![Expansion::Tilde { index: 0 }])),
                    Meta::Word(Word::new("foo~", vec![])),
                    Meta::Word(Word::new("bar/~", vec![])),
                    Meta::Word(Word::new("./~", vec![])),
                    Meta::Word(Word::new("~%", vec![])),
                    Meta::Word(Word::new("~baz", vec![])),
                ],
            })],
        };

        assert_eq!(expected, ast);
    }

    // FIXME: this probably requires (major?) changes to the lexing.
    //        it's making me wonder if the lexing part should be
    //        removed entirely, since lexing a POSIX shell-ish language
    //        seems really difficult
    #[test]
    fn nested_quotes_in_command_expansion_parsing() {
        let input = r#"echo "bat: $(cat "/sys/class/power_supply/BAT0/capacity")""#.to_string();
        let ast = parse(input);

        let expected = SyntaxTree {
            commands: vec![CommandType::Single(Command {
                name: Word::new("echo", vec![]),
                prefixes: vec![],
                suffixes: vec![Meta::Word(Word::new(
                    "bat: $(cat \"/sys/class/power_supply/BAT0/capacity\")",
                    vec![Expansion::Command {
                        range: 5..=50,
                        ast: SyntaxTree {
                            commands: vec![CommandType::Single(Command {
                                name: Word::new("cat", vec![]),
                                prefixes: vec![],
                                suffixes: vec![Meta::Word(Word::new(
                                    "/sys/class/power_supply/BAT0/capacity",
                                    vec![],
                                ))],
                            })],
                        },
                    }],
                ))],
            })],
        };

        assert_eq!(expected, ast);
    }

    #[test]
    fn nested_commands_parsing() {
        let input = r#"echo "foo: $(echo "$(whoami | lolcat)") yo""#.to_string();
        let ast = parse(input);

        let expected = SyntaxTree {
            commands: vec![CommandType::Single(Command {
                name: Word::new("echo", vec![]),
                prefixes: vec![],
                suffixes: vec![Meta::Word(Word::new(
                    r#"foo: $(echo "$(whoami | lolcat)") yo"#,
                    vec![Expansion::Command {
                        range: 5..=32,
                        ast: SyntaxTree {
                            commands: vec![CommandType::Single(Command {
                                name: Word::new("echo", vec![]),
                                prefixes: vec![],
                                suffixes: vec![Meta::Word(Word::new(
                                    "$(whoami | lolcat)",
                                    vec![Expansion::Command {
                                        range: 0..=17,
                                        ast: SyntaxTree {
                                            commands: vec![CommandType::Pipeline(vec![
                                                Command {
                                                    name: Word::new("whoami", vec![]),
                                                    prefixes: vec![],
                                                    suffixes: vec![],
                                                },
                                                Command {
                                                    name: Word::new("lolcat", vec![]),
                                                    prefixes: vec![],
                                                    suffixes: vec![],
                                                },
                                            ])],
                                        },
                                    }],
                                ))],
                            })],
                        },
                    }],
                ))],
            })],
        };

        assert_eq!(expected, ast);
    }

    #[test]
    fn command_expansion_without_quotes_parsing() {
        let input = "echo $(cat $(echo $(cat foo | rev) )) bar".to_string();
        let ast = parse(input);

        let expected = SyntaxTree {
            commands: vec![CommandType::Single(Command {
                name: Word::new("echo", vec![]),
                prefixes: vec![],
                suffixes: vec![
                    Meta::Word(Word::new(
                        "$(cat $(echo $(cat foo | rev) ))",
                        vec![Expansion::Command {
                            range: 0..=31,
                            ast: SyntaxTree {
                                commands: vec![CommandType::Single(Command {
                                    name: Word::new("cat", vec![]),
                                    prefixes: vec![],
                                    suffixes: vec![Meta::Word(Word::new(
                                        "$(echo $(cat foo | rev) )",
                                        vec![Expansion::Command {
                                            range: 0..=24,
                                            ast: SyntaxTree {
                                                commands: vec![CommandType::Single(Command {
                                                    name: Word::new("echo", vec![]),
                                                    prefixes: vec![],
                                                    suffixes: vec![Meta::Word(Word::new(
                                                        "$(cat foo | rev)",
                                                        vec![Expansion::Command {
                                                            range: 0..=15,
                                                            ast: SyntaxTree {
                                                                commands: vec![
                                                                    CommandType::Pipeline(vec![
                                                                        Command {
                                                                            name: Word::new(
                                                                                "cat",
                                                                                vec![],
                                                                            ),
                                                                            prefixes: vec![],
                                                                            suffixes: vec![
                                                                                Meta::Word(
                                                                                    Word::new(
                                                                                        "foo",
                                                                                        vec![],
                                                                                    ),
                                                                                ),
                                                                            ],
                                                                        },
                                                                        Command {
                                                                            name: Word::new(
                                                                                "rev",
                                                                                vec![],
                                                                            ),
                                                                            prefixes: vec![],
                                                                            suffixes: vec![],
                                                                        },
                                                                    ]),
                                                                ],
                                                            },
                                                        }],
                                                    ))],
                                                })],
                                            },
                                        }],
                                    ))],
                                })],
                            },
                        }],
                    )),
                    Meta::Word(Word::new("bar", vec![])),
                ],
            })],
        };

        assert_eq!(expected, ast);
    }

    #[test]
    fn multiple_nested_command_expansions_parsing() {
        let input = r#"echo "$(cat $(echo "$(cat foo)"))""#.to_string();
        let ast = parse(input);

        let expected = SyntaxTree {
            commands: vec![CommandType::Single(Command {
                name: Word::new("echo", vec![]),
                prefixes: vec![],
                suffixes: vec![Meta::Word(Word::new(
                    r#"$(cat $(echo "$(cat foo)"))"#,
                    vec![Expansion::Command {
                        range: 0..=26,
                        ast: SyntaxTree {
                            commands: vec![CommandType::Single(Command {
                                name: Word::new("cat", vec![]),
                                prefixes: vec![],
                                suffixes: vec![Meta::Word(Word::new(
                                    r#"$(echo "$(cat foo)")"#,
                                    vec![Expansion::Command {
                                        range: 0..=19,
                                        ast: SyntaxTree {
                                            commands: vec![CommandType::Single(Command {
                                                name: Word::new("echo", vec![]),
                                                prefixes: vec![],
                                                suffixes: vec![Meta::Word(Word::new(
                                                    "$(cat foo)",
                                                    vec![Expansion::Command {
                                                        range: 0..=9,
                                                        ast: SyntaxTree {
                                                            commands: vec![CommandType::Single(
                                                                Command {
                                                                    name: Word::new("cat", vec![]),
                                                                    prefixes: vec![],
                                                                    suffixes: vec![Meta::Word(
                                                                        Word::new("foo", vec![]),
                                                                    )],
                                                                },
                                                            )],
                                                        },
                                                    }],
                                                ))],
                                            })],
                                        },
                                    }],
                                ))],
                            })],
                        },
                    }],
                ))],
            })],
        };

        assert_eq!(expected, ast);
    }
}
