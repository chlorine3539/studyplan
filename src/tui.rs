//! Terminal UI: a "Today" to-do tab and a "Courses" progress tab.

use std::time::Duration;

use anyhow::Result;
use chrono::Local;
use ratatui::{
    DefaultTerminal, Frame,
    crossterm::event::{self, Event, KeyCode, KeyEventKind},
    layout::{Constraint, Layout, Rect},
    style::{Style, Stylize},
    widgets::{Block, Gauge, List, ListItem, ListState, Paragraph, Row, Table, TableState, Tabs},
};

use crate::{fmt, model::State, storage, youtube};

struct App {
    tab: usize,
    today: ListState,
    courses: TableState,
    /// `Some(course index)` while a course's video list is open (Courses tab).
    detail: Option<usize>,
    videos: ListState,
    status: String,
    quit: bool,
}

pub fn run(state: &mut State) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, state);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut DefaultTerminal, state: &mut State) -> Result<()> {
    let mut app = App {
        tab: 0,
        today: ListState::default().with_selected(Some(0)),
        courses: TableState::default().with_selected(Some(0)),
        detail: None,
        videos: ListState::default().with_selected(Some(0)),
        status: String::new(),
        quit: false,
    };
    while !app.quit {
        terminal.draw(|f| draw(f, &mut app, state))?;
        if !event::poll(Duration::from_millis(500))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if let Err(e) = handle_key(terminal, &mut app, state, key.code) {
            app.status = format!("error: {e:#}");
        }
    }
    Ok(())
}

fn handle_key(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    state: &mut State,
    code: KeyCode,
) -> Result<()> {
    app.status.clear();
    if let Some(ci) = app.detail {
        return handle_detail_key(app, state, ci, code);
    }
    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.quit = true,
        KeyCode::Tab | KeyCode::Right | KeyCode::Left | KeyCode::BackTab => {
            app.tab = (app.tab + 1) % 2
        }
        KeyCode::Char('1') => app.tab = 0,
        KeyCode::Char('2') => app.tab = 1,
        KeyCode::Down | KeyCode::Char('j') => move_selection(app, state, 1),
        KeyCode::Up | KeyCode::Char('k') => move_selection(app, state, -1),
        KeyCode::Enter if app.tab == 1 => {
            if let Some(i) = app.courses.selected() {
                open_detail(app, state, i);
            }
        }
        KeyCode::Char(' ') | KeyCode::Enter if app.tab == 0 => {
            if let Some(i) = app.today.selected() {
                state.toggle_today(i)?;
                storage::save(state)?;
            }
        }
        // Courses tab: how long a video of the selected course really takes you.
        KeyCode::Char(']') | KeyCode::Char('[') if app.tab == 1 => {
            if let Some(i) = app.courses.selected() {
                let step = if code == KeyCode::Char(']') { 0.25 } else { -0.25 };
                let next = state.courses[i].time_factor + step;
                state.set_time_factor(i, next)?;
                storage::save(state)?;
                app.status = format!(
                    "{}: ×{} study time",
                    state.courses[i].name, state.courses[i].time_factor
                );
            }
        }
        KeyCode::Char('r') => {
            state.refresh_today();
            storage::save(state)?;
            app.status = "re-planned today".into();
        }
        KeyCode::Char('+') | KeyCode::Char('=') => change_hours(app, state, 0.5)?,
        KeyCode::Char('-') => change_hours(app, state, -0.5)?,
        KeyCode::Char('s') => {
            let only = (app.tab == 1).then(|| app.courses.selected()).flatten();
            sync(terminal, app, state, only)?;
        }
        KeyCode::Char('S') => sync(terminal, app, state, None)?,
        _ => {}
    }
    clamp_selection(app, state);
    Ok(())
}

fn change_hours(app: &mut App, state: &mut State, delta: f64) -> Result<()> {
    state.set_daily_hours((state.daily_hours + delta).max(0.5))?;
    state.refresh_today();
    storage::save(state)?;
    app.status = format!("daily budget: {:.1}h", state.daily_hours);
    Ok(())
}

fn sync(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    state: &mut State,
    only: Option<usize>,
) -> Result<()> {
    // yt-dlp blocks, so draw the message first.
    app.status = "syncing with yt-dlp…".into();
    terminal.draw(|f| draw(f, app, state))?;

    let targets: Vec<usize> = match only {
        Some(i) => vec![i],
        None => (0..state.courses.len()).collect(),
    };
    let (mut added, mut removed, mut failed) = (0, 0, 0);
    let mut last_err = String::new();
    for i in targets {
        let url = state.courses[i].url.clone();
        match youtube::fetch_playlist(&url) {
            Ok(f) => {
                let m = state.upsert_course(f, &url, None);
                added += m.added;
                removed += m.removed;
            }
            Err(e) => {
                failed += 1;
                last_err = format!("{e:#}");
            }
        }
    }
    state.refresh_today();
    storage::save(state)?;
    app.status = if failed > 0 {
        format!("sync: {failed} failed ({last_err})")
    } else {
        format!("synced: +{added} new, -{removed} removed")
    };
    Ok(())
}

fn move_selection(app: &mut App, state: &State, delta: i32) {
    let len = if app.tab == 0 {
        state.today_items().len()
    } else {
        state.courses.len()
    };
    if len == 0 {
        return;
    }
    let cur = if app.tab == 0 {
        app.today.selected()
    } else {
        app.courses.selected()
    }
    .unwrap_or(0) as i32;
    let next = (cur + delta).rem_euclid(len as i32) as usize;
    if app.tab == 0 {
        app.today.select(Some(next));
    } else {
        app.courses.select(Some(next));
    }
}

fn clamp_selection(app: &mut App, state: &State) {
    let n = state.today_items().len();
    app.today
        .select(if n == 0 { None } else { Some(app.today.selected().unwrap_or(0).min(n - 1)) });
    let n = state.courses.len();
    app.courses
        .select(if n == 0 { None } else { Some(app.courses.selected().unwrap_or(0).min(n - 1)) });
}

fn draw(f: &mut Frame, app: &mut App, state: &State) {
    let [tabs_area, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        Constraint::Length(2),
    ])
    .areas(f.area());

    let tabs = Tabs::new(["1 Today", "2 Courses"])
        .select(app.tab)
        .block(Block::bordered().title(" studyplan "))
        .highlight_style(Style::new().yellow().bold());
    f.render_widget(tabs, tabs_area);

    if app.tab == 1 && app.detail.is_some() {
        draw_detail(f, app, state, body);
    } else if app.tab == 0 {
        draw_today(f, app, state, body);
    } else {
        draw_courses(f, app, state, body);
    }

    let help = match (app.tab, app.detail.is_some()) {
        (_, true) => {
            "↑↓ move · space toggle watched · m mark up to here · u unmark from here · esc back · q quit"
        }
        (0, false) => {
            "↑↓ move · space check off · r re-plan · +/- hours · S sync all · tab switch · q quit"
        }
        _ => "↑↓ move · enter videos · [ ] study-time factor · s sync · S sync all · +/- hours · q quit",
    };
    let status = if app.status.is_empty() {
        String::new()
    } else {
        app.status.clone()
    };
    f.render_widget(
        Paragraph::new(vec![status.yellow().into(), help.dark_gray().into()]),
        footer,
    );
}

fn draw_today(f: &mut Frame, app: &mut App, state: &State, area: Rect) {
    let items = state.today_items();
    let planned: u64 = items
        .iter()
        .map(|(c, v)| c.study_secs(v.duration_secs))
        .sum();
    let done: u64 = items
        .iter()
        .filter(|(_, v)| v.watched)
        .map(|(c, v)| c.study_secs(v.duration_secs))
        .sum();
    let ratio = if planned == 0 {
        0.0
    } else {
        (done as f64 / planned as f64).clamp(0.0, 1.0)
    };

    let [gauge_area, list_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).areas(area);

    let gauge = Gauge::default()
        .block(Block::bordered().title(format!(
            " {} · budget {:.1}h · planned {} ",
            Local::now().format("%a %d %b"),
            state.daily_hours,
            fmt::hm(planned)
        )))
        .gauge_style(Style::new().green())
        .ratio(ratio)
        .label(format!("{} / {}", fmt::hm(done), fmt::hm(planned)));
    f.render_widget(gauge, gauge_area);

    if items.is_empty() {
        let msg = if state.courses.is_empty() {
            "No courses yet. Quit and run: studyplan add <playlist-url>"
        } else {
            "Everything is watched. Nice!"
        };
        f.render_widget(
            Paragraph::new(msg).block(Block::bordered().title(" To-do ")),
            list_area,
        );
        return;
    }

    let rows: Vec<ListItem> = items
        .iter()
        .map(|(c, v)| {
            let text = format!(
                "{} {} · {} ({})",
                if v.watched { "[x]" } else { "[ ]" },
                c.name,
                v.title,
                c.label(v)
            );
            let style = if v.watched {
                Style::new().dark_gray().crossed_out()
            } else {
                Style::new()
            };
            ListItem::new(text).style(style)
        })
        .collect();
    let list = List::new(rows)
        .block(Block::bordered().title(" To-do "))
        .highlight_style(Style::new().reversed())
        .highlight_symbol("▶ ");
    f.render_stateful_widget(list, list_area, &mut app.today);
}

fn draw_courses(f: &mut Frame, app: &mut App, state: &State, area: Rect) {
    let [table_area, info_area] =
        Layout::vertical([Constraint::Min(4), Constraint::Length(4)]).areas(area);

    let header = Row::new([
        "#",
        "Course",
        "Videos",
        "Watched",
        "Total",
        "×",
        "Study left",
        "Progress",
    ])
    .style(Style::new().yellow().bold());
    let rows: Vec<Row> = state
        .courses
        .iter()
        .enumerate()
        .map(|(i, c)| {
            Row::new(vec![
                (i + 1).to_string(),
                c.name.clone(),
                format!("{}/{}", c.watched_count(), c.videos.len()),
                fmt::hm(c.watched_secs()),
                fmt::hm(c.total_secs()),
                format!("×{}", c.time_factor),
                fmt::hm(c.study_secs(c.total_secs() - c.watched_secs())),
                fmt::bar(c.percent(), 20),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Min(18),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(5),
            Constraint::Length(10),
            Constraint::Length(28),
        ],
    )
    .header(header)
    .block(Block::bordered().title(" Courses (progress by watch time) "))
    .row_highlight_style(Style::new().reversed());
    f.render_stateful_widget(table, table_area, &mut app.courses);

    // Overall progress is weighted by study time, so a course that takes you
    // twice as long per video counts twice as much.
    let total: u64 = state
        .courses
        .iter()
        .map(|c| c.study_secs(c.total_secs()))
        .sum();
    let watched: u64 = state
        .courses
        .iter()
        .map(|c| c.study_secs(c.watched_secs()))
        .sum();
    let overall = if total == 0 {
        0.0
    } else {
        watched as f64 * 100.0 / total as f64
    };
    let eta = match state.eta_days() {
        Some(d) => {
            let finish = Local::now().date_naive() + chrono::Duration::days(d as i64 - 1);
            format!(
                "At {:.1}h/day: about {d} day(s) left (around {finish}), assuming you hit the budget every day",
                state.daily_hours
            )
        }
        None => "Nothing left to watch".to_string(),
    };
    f.render_widget(
        Paragraph::new(vec![
            format!(
                "Overall: {} ({} of {} study time)",
                fmt::bar(overall, 20),
                fmt::hm(watched),
                fmt::hm(total)
            )
            .into(),
            eta.into(),
        ])
        .block(Block::bordered().title(" Overall ")),
        info_area,
    );
}

/// Open a course's video list with the cursor on its first unwatched video.
fn open_detail(app: &mut App, state: &State, course: usize) {
    let Some(c) = state.courses.get(course) else {
        return;
    };
    let first_unwatched = c.videos.iter().position(|v| !v.watched).unwrap_or(0);
    app.videos.select(Some(first_unwatched));
    app.detail = Some(course);
}

fn handle_detail_key(app: &mut App, state: &mut State, ci: usize, code: KeyCode) -> Result<()> {
    let Some(n) = state.courses.get(ci).map(|c| c.videos.len()) else {
        app.detail = None;
        return Ok(());
    };
    let sel = app.videos.selected().unwrap_or(0).min(n.saturating_sub(1));
    match code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Esc | KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => app.detail = None,
        KeyCode::Down | KeyCode::Char('j') if n > 0 => app.videos.select(Some((sel + 1) % n)),
        KeyCode::Up | KeyCode::Char('k') if n > 0 => app.videos.select(Some((sel + n - 1) % n)),
        KeyCode::Char(' ') | KeyCode::Enter if n > 0 => {
            let now = state.courses[ci].videos[sel].watched;
            state.set_videos_watched(ci, &[sel], !now);
            storage::save(state)?;
        }
        // "I've seen everything up to here": the common case for a course you started earlier.
        KeyCode::Char('m') if n > 0 => {
            let idx: Vec<usize> = (0..=sel).collect();
            state.set_videos_watched(ci, &idx, true);
            storage::save(state)?;
            app.status = format!("marked videos 1-{} as watched", sel + 1);
        }
        KeyCode::Char('u') if n > 0 => {
            let idx: Vec<usize> = (sel..n).collect();
            state.set_videos_watched(ci, &idx, false);
            storage::save(state)?;
            app.status = format!("marked videos {}-{} as not watched", sel + 1, n);
        }
        _ => {}
    }
    Ok(())
}

fn draw_detail(f: &mut Frame, app: &mut App, state: &State, area: Rect) {
    let Some(c) = app.detail.and_then(|i| state.courses.get(i)) else {
        return;
    };
    let rows: Vec<ListItem> = c
        .videos
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let text = format!(
                "{} {:>3}. {} ({})",
                if v.watched { "[x]" } else { "[ ]" },
                i + 1,
                v.title,
                c.label(v)
            );
            let style = if v.watched {
                Style::new().dark_gray()
            } else {
                Style::new()
            };
            ListItem::new(text).style(style)
        })
        .collect();
    let list = List::new(rows)
        .block(Block::bordered().title(format!(
            " {} · {}/{} watched · {} ",
            c.name,
            c.watched_count(),
            c.videos.len(),
            fmt::bar(c.percent(), 10)
        )))
        .highlight_style(Style::new().reversed())
        .highlight_symbol("▶ ");
    f.render_stateful_widget(list, area, &mut app.videos);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Course, Mode, Video};
    use ratatui::{Terminal, backend::TestBackend};

    fn sample() -> State {
        let vid = |id: &str, t: &str, m: u64, w: bool| Video {
            id: id.into(),
            title: t.into(),
            duration_secs: m * 60,
            watched: w,
        };
        let mut s = State::default();
        s.mode = Mode::Interleave;
        s.courses = vec![
            Course {
                id: "a".into(),
                name: "Rust Crash Course".into(),
                url: String::new(),
                videos: vec![vid("a1", "Intro", 15, false), vid("a2", "Ownership", 40, false), vid("a3", "Traits", 45, false)],
                time_factor: 2.0,
            },
            Course {
                id: "b".into(),
                name: "Linux Basics".into(),
                url: String::new(),
                videos: vec![vid("b1", "Filesystem", 20, false)],
                time_factor: 1.0,
            },
        ];
        s.daily_hours = 1.5;
        s.refresh_today();
        // Watch the first item the way a user would: through today's list.
        s.set_watched_today(0, true).unwrap();
        s
    }

    fn render(tab: usize, detail: Option<usize>) -> String {
        let state = sample();
        let mut app = App {
            tab,
            today: ListState::default().with_selected(Some(0)),
            courses: TableState::default().with_selected(Some(0)),
            detail,
            videos: ListState::default().with_selected(Some(0)),
            status: "hello".into(),
            quit: false,
        };
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| draw(f, &mut app, &state)).unwrap();
        format!("{}", term.backend())
    }

    #[test]
    fn today_tab_renders() {
        let out = render(0, None);
        println!("{out}");
        assert!(out.contains("Rust Crash Course · Intro"));
        assert!(out.contains("[x]"));
        assert!(out.contains("budget 1.5h"));
        // Rust has a factor of 2: a 15:00 video costs 30 minutes of the day.
        assert!(out.contains("15:00 → 30m"));
    }

    #[test]
    fn courses_tab_renders() {
        let out = render(1, None);
        println!("{out}");
        assert!(out.contains("Linux Basics"));
        assert!(out.contains("1/3"));
        assert!(out.contains("Overall"));
        assert!(out.contains("×2"));
        assert!(out.contains("Study left"));
    }

    #[test]
    fn video_list_renders() {
        let out = render(1, Some(0));
        println!("{out}");
        assert!(out.contains("Rust Crash Course · 1/3 watched"));
        assert!(out.contains("[x]   1. Intro (15:00 → 30m)"));
        assert!(out.contains("[ ]   2. Ownership (40:00 → 1h 20m)"));
        assert!(out.contains("m mark up to here"));
    }

    #[test]
    fn mark_up_to_here_and_unmark() {
        let mut state = sample();
        let mut app = App {
            tab: 1,
            today: ListState::default(),
            courses: TableState::default().with_selected(Some(0)),
            detail: Some(0),
            videos: ListState::default().with_selected(Some(1)),
            status: String::new(),
            quit: false,
        };
        // Cursor on video 2 ("Ownership"): `m` marks videos 1-2 as watched.
        // (Writes to the real data path unless STUDYPLAN_DATA is set, so point it at a temp file.)
        let tmp = std::env::temp_dir().join("studyplan-test-state.json");
        // SAFETY: tests in this module that touch the env var all set the same value.
        unsafe { std::env::set_var("STUDYPLAN_DATA", &tmp) };
        handle_detail_key(&mut app, &mut state, 0, KeyCode::Char('m')).unwrap();
        assert_eq!(state.courses[0].watched_count(), 2);
        assert_eq!(app.status, "marked videos 1-2 as watched");
        // `u` on video 2 unmarks videos 2-3 again.
        handle_detail_key(&mut app, &mut state, 0, KeyCode::Char('u')).unwrap();
        assert_eq!(state.courses[0].watched_count(), 1);
        // Esc closes the list.
        handle_detail_key(&mut app, &mut state, 0, KeyCode::Esc).unwrap();
        assert!(app.detail.is_none());
        let _ = std::fs::remove_file(tmp);
    }
}
