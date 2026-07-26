//! Drawing the console.
//!
//! Everything here reads [`Cli`] and writes to the frame; nothing decides
//! anything. If a screen looks wrong, the state is wrong.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table,
    TableState, Wrap,
};
use ratatui::Frame;
use serde_json::Value;

use super::api::scalar;
use super::state::{
    Cli, Focus, Form, Main, NavKind, Onboarding, SignIn, Team, PAGE, SIGN_IN_OPTIONS,
};

const ACCENT: Color = Color::Cyan;
const FAINT: Color = Color::DarkGray;
const BAD: Color = Color::Red;
const GOOD: Color = Color::Green;

pub fn draw(frame: &mut Frame, cli: &Cli) {
    let area = frame.area();

    if cli.sign_in.is_some() {
        draw_sign_in(frame, cli, area);
    } else {
        // An error gets its own bordered row rather than a slot in the status
        // line: a one-line strip at the bottom of the screen is where messages
        // go to be missed, and the ones that matter here are the ones saying
        // why something you just pressed did nothing.
        let banner = error_height(cli, area.width);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(banner),
                Constraint::Length(1),
            ])
            .split(area);
        draw_header(frame, cli, rows[0]);

        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(28), Constraint::Min(20)])
            .split(rows[1]);
        draw_nav(frame, cli, columns[0]);
        draw_main(frame, cli, columns[1]);
        if banner > 0 {
            draw_error(frame, cli, rows[2]);
        }
        draw_status(frame, cli, rows[3]);
    }

    if let Some(onboarding) = &cli.onboarding {
        let area = centred(area, 68, 70);
        frame.render_widget(Clear, area);
        match onboarding {
            Onboarding::Create(form) => draw_form(frame, form, area, true, cli.error.as_deref()),
            Onboarding::AskAnAdmin => {
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from(
                            "Your account is set up, but it does not belong to a workspace — and \
                             almost everything here lives inside one.",
                        ),
                        Line::from(""),
                        Line::from(
                            "This app does not let members start their own organization, so an \
                             administrator has to add you. Ask someone who already administers \
                             one to invite you from their Team screen.",
                        ),
                        Line::from(""),
                        Line::from(vec![
                            Span::styled("r", Style::default().fg(ACCENT).bold()),
                            Span::raw(" check again    "),
                            Span::styled("x", Style::default().fg(ACCENT).bold()),
                            Span::raw(" sign out    "),
                            Span::styled("esc", Style::default().fg(ACCENT).bold()),
                            Span::raw(" carry on anyway"),
                        ]),
                    ])
                    .wrap(Wrap { trim: true })
                    .block(popup("You are not in an organization yet")),
                    area,
                );
            }
        }
    }

    if let Some(picker) = &cli.picker {
        let items: Vec<ListItem> = picker
            .items
            .iter()
            .map(|(_, label)| ListItem::new(label.clone()))
            .collect();
        let area = centred(area, 60, 60);
        frame.render_widget(Clear, area);
        let mut state = ListState::default().with_selected(Some(picker.index));
        frame.render_stateful_widget(
            List::new(items)
                .block(popup(&picker.title))
                .highlight_style(Style::default().fg(Color::Black).bg(ACCENT))
                .highlight_symbol("› "),
            area,
            &mut state,
        );
    }

    if let Some(confirm) = &cli.confirm {
        let area = centred(area, 60, 30);
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(confirm.prompt.clone()),
                Line::from(""),
                Line::from(vec![
                    Span::styled("y", Style::default().fg(ACCENT).bold()),
                    Span::raw(" yes    "),
                    Span::styled("n", Style::default().fg(ACCENT).bold()),
                    Span::raw(" no"),
                ]),
            ])
            .wrap(Wrap { trim: true })
            .block(popup("Are you sure?")),
            area,
        );
    }

    if cli.help {
        let area = centred(area, 70, 80);
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(help_text())
                .block(popup("Keys"))
                .wrap(Wrap { trim: false }),
            area,
        );
    }
}

/// How many rows the error banner needs, or 0 when there is nothing to say.
fn error_height(cli: &Cli, width: u16) -> u16 {
    let Some(error) = &cli.error else { return 0 };
    // Two for the border, then however many the message wraps to — capped, so a
    // wall of text from a server cannot push the interface off the screen.
    let usable = width.saturating_sub(4).max(1) as usize;
    let lines = error.chars().count().div_ceil(usable).clamp(1, 5);
    lines as u16 + 2
}

fn draw_error(frame: &mut Frame, cli: &Cli, area: Rect) {
    let Some(error) = &cli.error else { return };
    frame.render_widget(
        Paragraph::new(error.clone())
            .style(Style::default().fg(BAD))
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(BAD))
                    .title(" Error "),
            ),
        area,
    );
}

// --- chrome ----------------------------------------------------------------

fn draw_header(frame: &mut Frame, cli: &Cli, area: Rect) {
    let title = if cli.manifest.app_name.is_empty() {
        "apiplant".to_string()
    } else {
        cli.manifest.app_name.clone()
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" {title} "),
            Style::default().fg(Color::Black).bg(ACCENT).bold(),
        ),
        Span::raw(" "),
        Span::styled(cli.client.origin.clone(), Style::default().fg(FAINT)),
        Span::styled("  ·  ", Style::default().fg(FAINT)),
        Span::styled(cli.identity_label(), Style::default().fg(FAINT)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_status(frame: &mut Frame, cli: &Cli, area: Rect) {
    // Errors have their own row above this one; the status line is for the
    // running commentary, which nobody needs to catch.
    let message = format!(" {}", cli.status);
    let style = Style::default().fg(FAINT);
    let right = format!("{}  ·  ? keys  ·  q quit ", cli.organization_label());

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(10),
            Constraint::Length(right.len() as u16 + 1),
        ])
        .split(area);
    frame.render_widget(Paragraph::new(message).style(style), columns[0]);
    frame.render_widget(
        Paragraph::new(right)
            .alignment(Alignment::Right)
            .style(Style::default().fg(FAINT)),
        columns[1],
    );
}

fn draw_nav(frame: &mut Frame, cli: &Cli, area: Rect) {
    let mut items: Vec<ListItem> = Vec::new();
    // The list is flat so the selection index matches `nav_index`; group names
    // are rendered as part of the first item in each group instead of as their
    // own rows, which would make every index arithmetic off by a heading.
    let mut previous: Option<&str> = None;
    for item in &cli.nav {
        let mut lines = Vec::new();
        if previous != Some(item.group.as_str()) {
            lines.push(Line::from(Span::styled(
                item.group.to_uppercase(),
                Style::default().fg(FAINT).add_modifier(Modifier::DIM),
            )));
            previous = Some(item.group.as_str());
        }
        lines.push(Line::from(format!(" {}", item.label)));
        items.push(ListItem::new(Text::from(lines)));
    }

    let focused = cli.focus == Focus::Nav;
    let mut state = ListState::default().with_selected(Some(cli.nav_index));
    frame.render_stateful_widget(
        List::new(items)
            .block(pane("Navigate", focused))
            .highlight_style(if focused {
                Style::default().fg(Color::Black).bg(ACCENT)
            } else {
                Style::default().add_modifier(Modifier::REVERSED)
            }),
        area,
        &mut state,
    );
}

// --- the main pane ----------------------------------------------------------

fn draw_main(frame: &mut Frame, cli: &Cli, area: Rect) {
    let focused = cli.focus == Focus::Main;
    match &cli.main {
        Main::Empty(message) => {
            frame.render_widget(
                Paragraph::new(message.clone())
                    .style(Style::default().fg(FAINT))
                    .block(pane("apiplant", focused))
                    .wrap(Wrap { trim: true }),
                area,
            );
        }
        Main::List(list) => draw_list(frame, cli, list, area, focused),
        Main::Detail(detail) => {
            let resource = cli.resource(detail.resource);
            let title = resource
                .map(|resource| {
                    format!("{} — {}", resource.label, resource.title_of(&detail.record))
                })
                .unwrap_or_else(|| "Record".into());
            let mut lines = Vec::new();
            for (name, value) in super::api::as_object(&detail.record) {
                let label = resource
                    .and_then(|resource| resource.field(&name))
                    .map(|field| field.label.clone())
                    .unwrap_or_else(|| name.clone());
                lines.push(Line::from(vec![
                    Span::styled(format!("{label:>22}  "), Style::default().fg(FAINT)),
                    Span::raw(long(&value)),
                ]));
            }
            if lines.is_empty() {
                lines.push(Line::from(Span::styled(
                    "(empty)",
                    Style::default().fg(FAINT),
                )));
            }
            frame.render_widget(
                Paragraph::new(lines)
                    .scroll((detail.scroll, 0))
                    .block(pane(&title, focused))
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
        Main::Form(form) => draw_form(frame, form, area, focused, None),
        Main::Output {
            title,
            body,
            scroll,
        } => {
            frame.render_widget(
                Paragraph::new(body.clone())
                    .scroll((*scroll, 0))
                    .block(pane(title, focused))
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
        Main::Team(team) => draw_team(frame, cli, team, area, focused),
        Main::Session => draw_session(frame, cli, area, focused),
    }
}

fn draw_list(frame: &mut Frame, cli: &Cli, list: &super::state::List, area: Rect, focused: bool) {
    let Some(resource) = cli.resource(list.resource) else {
        return;
    };

    let title = format!(
        "{}  ·  page {}{}",
        resource.plural,
        list.page + 1,
        if list.search.is_empty() {
            String::new()
        } else {
            format!(
                "  ·  {} = {}",
                resource.search_field.clone().unwrap_or_default(),
                list.search
            )
        }
    );
    let block = pane(&title, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows_area = if list.searching {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(inner);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("search ", Style::default().fg(ACCENT)),
                Span::raw(list.search.clone()),
                Span::styled("▏", Style::default().fg(ACCENT)),
            ])),
            split[0],
        );
        split[1]
    } else {
        inner
    };

    if list.rows.is_empty() {
        frame.render_widget(
            Paragraph::new(if list.search.is_empty() {
                format!(
                    "No {} yet. Press n to create one.",
                    resource.plural.to_lowercase()
                )
            } else {
                "Nothing matched that search.".to_string()
            })
            .style(Style::default().fg(FAINT)),
            rows_area,
        );
        return;
    }

    // The manifest names the columns the dashboard shows; falling back to the
    // first few visible fields keeps a resource that named none from rendering
    // as a table of nothing.
    let columns: Vec<String> = if resource.columns.is_empty() {
        resource
            .fields
            .iter()
            .filter(|field| field.admin_visible && !field.hidden)
            .take(5)
            .map(|field| field.name.clone())
            .collect()
    } else {
        resource.columns.clone()
    };

    let header = Row::new(
        columns
            .iter()
            .map(|name| {
                let label = resource
                    .field(name)
                    .map(|field| field.label.clone())
                    .unwrap_or_else(|| name.clone());
                Cell::from(label)
            })
            .collect::<Vec<_>>(),
    )
    .style(Style::default().fg(FAINT).add_modifier(Modifier::BOLD));

    let body: Vec<Row> = list
        .rows
        .iter()
        .map(|record| {
            Row::new(
                columns
                    .iter()
                    .map(|name| Cell::from(cell(record.get(name))))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();

    let widths = vec![Constraint::Ratio(1, columns.len().max(1) as u32); columns.len()];
    let mut state = TableState::default().with_selected(Some(list.index));
    frame.render_stateful_widget(
        Table::new(body, widths)
            .header(header)
            .row_highlight_style(Style::default().fg(Color::Black).bg(ACCENT))
            .highlight_symbol("› "),
        rows_area,
        &mut state,
    );
}

fn draw_form(frame: &mut Frame, form: &Form, area: Rect, focused: bool, error: Option<&str>) {
    let block = pane(&form.title, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    if let Some(subtitle) = &form.subtitle {
        lines.push(Line::from(Span::styled(
            subtitle.clone(),
            Style::default().fg(FAINT),
        )));
        lines.push(Line::from(""));
    }

    for (index, field) in form.fields.iter().enumerate() {
        let selected = index == form.index;
        let editing = selected && form.editing;
        let marker = if selected { "› " } else { "  " };
        let shown = if field.secret && !field.value.is_empty() {
            "•".repeat(field.value.chars().count())
        } else {
            field.value.clone()
        };

        lines.push(Line::from(vec![Span::styled(
            format!(
                "{marker}{}{}",
                field.label,
                if field.required { " *" } else { "" }
            ),
            if selected {
                Style::default().fg(ACCENT).bold()
            } else {
                Style::default().fg(FAINT)
            },
        )]));
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(
                if shown.is_empty() && !editing {
                    placeholder(field).to_string()
                } else {
                    shown
                },
                if editing {
                    Style::default().fg(Color::Black).bg(ACCENT)
                } else if field.value.is_empty() {
                    Style::default().fg(FAINT).add_modifier(Modifier::DIM)
                } else {
                    Style::default()
                },
            ),
            Span::styled(if editing { "▏" } else { "" }, Style::default().fg(ACCENT)),
        ]));
        if selected {
            if let Some(help) = &field.help {
                lines.push(Line::from(Span::styled(
                    format!("    {help}"),
                    Style::default().fg(FAINT),
                )));
            }
        }
        lines.push(Line::from(""));
    }

    let on_submit = form.on_submit();
    lines.push(Line::from(Span::styled(
        format!("{}[ {} ]", if on_submit { "› " } else { "  " }, form.submit),
        if on_submit {
            Style::default().fg(Color::Black).bg(GOOD).bold()
        } else {
            Style::default().fg(GOOD)
        },
    )));
    // The message belongs next to the button that produced it. An overlay covers
    // the status line's half of the screen, and "nothing happened" is exactly
    // how an unread error reads.
    if let Some(error) = error {
        lines.push(Line::from(""));
        // The message, the request that produced it and its status are separate
        // lines; running them together is how "not accepted" ends up welded to
        // a URL.
        for line in error.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {line}"),
                Style::default().fg(BAD).bold(),
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        if form.editing {
            "  enter done  ·  esc cancel"
        } else {
            "  ↑↓ move  ·  enter edit or pick  ·  space toggle  ·  D clear  ·  esc back"
        },
        Style::default().fg(FAINT),
    )));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn placeholder(field: &super::state::FormField) -> &'static str {
    if field.references.is_some() {
        "(enter to pick)"
    } else if field.required {
        "(required)"
    } else {
        "(empty)"
    }
}

/// Who is in the organisation and what each of them may do.
fn draw_team(frame: &mut Frame, cli: &Cli, team: &Team, area: Rect, focused: bool) {
    let block = pane(&format!("Team  ·  {}", cli.organization_label()), focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if team.members.is_empty() {
        frame.render_widget(
            Paragraph::new("Nobody to show here.").style(Style::default().fg(FAINT)),
            inner,
        );
        return;
    }

    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);

    let header = Row::new(vec![Cell::from("Member"), Cell::from("Roles")])
        .style(Style::default().fg(FAINT).add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = team
        .members
        .iter()
        .map(|member| {
            let mut name = member.name.clone();
            if name.is_empty() {
                name = "unnamed".into();
            }
            if member.is_me {
                name.push_str("  (you)");
            }
            // An admin holds every role the app defines, so listing their
            // stored roles alone would understate what they can do.
            let roles = if member.roles().is_empty() {
                Line::from(Span::styled("no role", Style::default().fg(FAINT)))
            } else {
                let mut spans: Vec<Span> = Vec::new();
                for role in member.roles() {
                    if !spans.is_empty() {
                        spans.push(Span::raw("  "));
                    }
                    let style = if role == "admin" {
                        Style::default().fg(GOOD).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(ACCENT)
                    };
                    spans.push(Span::styled(role, style));
                }
                if member.roles().iter().any(|role| role == "admin") {
                    spans.push(Span::styled(
                        "  (and every other role)",
                        Style::default().fg(FAINT),
                    ));
                }
                Line::from(spans)
            };
            Row::new(vec![Cell::from(name), Cell::from(roles)])
        })
        .collect();

    let mut state = TableState::default().with_selected(Some(team.index));
    frame.render_stateful_widget(
        Table::new(rows, [Constraint::Ratio(2, 5), Constraint::Ratio(3, 5)])
            .header(header)
            .row_highlight_style(Style::default().fg(Color::Black).bg(ACCENT))
            .highlight_symbol("› "),
        split[0],
        &mut state,
    );

    let hint = if team.manage {
        "  g  give a role      d  take one away      r  reload"
    } else {
        "  You may not change roles in this organization."
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(hint, Style::default().fg(FAINT))),
        ]),
        split[1],
    );
}

fn draw_session(frame: &mut Frame, cli: &Cli, area: Rect, focused: bool) {
    let key = match &cli.client.credentials.api_key {
        Some(key) => format!("{}… (saved for this server)", &key[..key.len().min(8)]),
        None if cli.client.credentials.token.is_some() => {
            "a session token — nothing saved, this ends when you quit".into()
        }
        None => "none".into(),
    };
    let lines = vec![
        row("App", &cli.manifest.app_name),
        row("Directory", &cli.dir.display().to_string()),
        row("Signed in as", &cli.identity_label()),
        row(
            "User id",
            cli.identity_id
                .as_deref()
                .map(str::to_string)
                .or_else(|| {
                    cli.identity_note.clone().map(|note| {
                        format!("unknown — {}", note.lines().next().unwrap_or_default())
                    })
                })
                .unwrap_or_else(|| "unknown".into())
                .as_str(),
        ),
        row("Server", &cli.client.origin),
        row(
            "API",
            &format!("{}{}", cli.client.origin, cli.client.base_path),
        ),
        row("Dashboard", &cli.client.admin_url()),
        row("Credential", &key),
        row("Organization", &cli.organization_label()),
        row(
            "Your roles",
            &if cli.roles.is_empty() {
                "none here".to_string()
            } else if cli.roles.iter().any(|role| role == "admin") {
                format!("{} — an admin holds every role", cli.roles.join(", "))
            } else {
                cli.roles.join(", ")
            },
        ),
        row(
            "Resources",
            &format!(
                "{} listed, {} actions",
                cli.nav
                    .iter()
                    .filter(|i| matches!(i.kind, NavKind::Resource(_)))
                    .count(),
                cli.nav
                    .iter()
                    .filter(|i| matches!(i.kind, NavKind::Function(_)))
                    .count(),
            ),
        ),
        Line::from(""),
        Line::from(Span::styled(
            "  g  issue a new API key and show it",
            Style::default().fg(FAINT),
        )),
        Line::from(Span::styled(
            "  O  switch organization",
            Style::default().fg(FAINT),
        )),
        Line::from(Span::styled(
            "  N  create an organization",
            Style::default().fg(FAINT),
        )),
        Line::from(Span::styled(
            "  x  sign out and forget the saved key",
            Style::default().fg(FAINT),
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(pane("Session", focused))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn row<'a>(label: &'a str, value: &str) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{label:>14}  "), Style::default().fg(FAINT)),
        Span::raw(value.to_string()),
    ])
}

// --- signing in --------------------------------------------------------------

fn draw_sign_in(frame: &mut Frame, cli: &Cli, area: Rect) {
    let area = centred(area, 78, 70);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(format!(
            " {} · {} ",
            if cli.manifest.app_name.is_empty() {
                "apiplant"
            } else {
                &cli.manifest.app_name
            },
            cli.client.origin
        ))
        .border_style(Style::default().fg(ACCENT));
    let inner = block.inner(area).inner(Margin::new(1, 1));
    frame.render_widget(block, area);

    match cli.sign_in.as_ref() {
        Some(SignIn::Menu { index }) => {
            let mut lines = vec![
                Line::from(Span::styled(
                    "Connect this console to the app.",
                    Style::default().bold(),
                )),
                Line::from(""),
            ];
            for (at, (title, description)) in SIGN_IN_OPTIONS.iter().enumerate() {
                let selected = at == *index;
                lines.push(Line::from(Span::styled(
                    format!("{}{title}", if selected { "› " } else { "  " }),
                    if selected {
                        Style::default().fg(ACCENT).bold()
                    } else {
                        Style::default()
                    },
                )));
                lines.push(Line::from(Span::styled(
                    format!("  {description}"),
                    Style::default().fg(FAINT),
                )));
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                "↑↓ choose  ·  enter select  ·  q quit",
                Style::default().fg(FAINT),
            )));
            if let Some(error) = &cli.error {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    error.clone(),
                    Style::default().fg(BAD),
                )));
            }
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
        Some(SignIn::Form(form)) => draw_form(frame, form, inner, true, cli.error.as_deref()),
        Some(SignIn::Browser { url, opened, .. }) => {
            let lines = vec![
                Line::from(Span::styled(
                    "Waiting for the dashboard…",
                    Style::default().fg(ACCENT).bold(),
                )),
                Line::from(""),
                Line::from(if *opened {
                    "A browser tab was opened. Sign in there and it will send a key back."
                } else {
                    "Open this address in a browser, sign in, and it will send a key back."
                }),
                Line::from(""),
                Line::from(Span::styled(url.clone(), Style::default().fg(GOOD))),
                Line::from(""),
                Line::from(Span::styled(
                    "o open it again  ·  esc go back  ·  q quit",
                    Style::default().fg(FAINT),
                )),
            ];
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
        None => {}
    }
}

// --- bits -------------------------------------------------------------------

fn pane(title: &str, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(format!(" {title} "))
        .border_style(Style::default().fg(if focused { ACCENT } else { FAINT }))
}

fn popup(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(format!(" {title} "))
        .border_style(Style::default().fg(ACCENT))
        .padding(ratatui::widgets::Padding::new(1, 1, 1, 1))
}

/// A percentage-sized box in the middle of `area`.
fn centred(area: Rect, width: u16, height: u16) -> Rect {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height) / 2),
            Constraint::Percentage(height),
            Constraint::Percentage((100 - height) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width) / 2),
            Constraint::Percentage(width),
            Constraint::Percentage((100 - width) / 2),
        ])
        .split(rows[1])[1]
}

/// One table cell: a single line, however nested the value is.
fn cell(value: Option<&Value>) -> String {
    let text = value.map(scalar).unwrap_or_default();
    let text = text.replace(['\n', '\r'], " ");
    if text.chars().count() > 60 {
        format!("{}…", text.chars().take(59).collect::<String>())
    } else {
        text
    }
}

/// A detail-screen value, where nested JSON is worth seeing in full.
fn long(value: &Value) -> String {
    match value {
        Value::Object(_) | Value::Array(_) => {
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
        }
        other => scalar(other),
    }
}

fn help_text() -> Text<'static> {
    let section = |title: &str| {
        Line::from(Span::styled(
            title.to_string(),
            Style::default().fg(ACCENT).bold(),
        ))
    };
    let key = |keys: &str, what: &str| {
        Line::from(vec![
            Span::styled(format!("  {keys:<16}"), Style::default().fg(GOOD)),
            Span::raw(what.to_string()),
        ])
    };
    Text::from(vec![
        section("Anywhere"),
        key("tab", "move between the sidebar and the pane"),
        key("O", "switch organization"),
        key("?", "this list"),
        key("q / ctrl-c", "quit"),
        Line::from(""),
        section("Lists"),
        key("↑ ↓ / k j", "move"),
        key("enter", "open the record"),
        key("n", "new record"),
        key("e", "edit"),
        key("d", "delete"),
        key("/", "search"),
        key("[ ]", format!("previous / next page of {PAGE}").as_str()),
        key("r", "reload"),
        Line::from(""),
        section("Forms and actions"),
        key("↑ ↓", "move between fields"),
        key("enter", "edit a field, pick a reference, or submit"),
        key("space", "toggle a switch"),
        key("D", "clear a field"),
        key("esc", "leave the form"),
        Line::from(""),
        section("Team screen"),
        key("↑ ↓ / k j", "move between members"),
        key("g", "give the highlighted member a role"),
        key("d", "take one of their roles away"),
        key("r", "reload"),
        Line::from(""),
        section("Session screen"),
        key("g", "issue an API key"),
        key("N", "create an organization"),
        key("x", "sign out"),
    ])
}
