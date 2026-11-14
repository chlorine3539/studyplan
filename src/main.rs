mod fmt;
mod model;
mod planner;
mod storage;
mod tui;
mod youtube;

use anyhow::{Result, bail};
use chrono::{Duration, Local};
use clap::{Parser, Subcommand};

use model::{Mode, State};

#[derive(Parser)]
#[command(name = "studyplan", version, about = "Turn YouTube course playlists into a daily study to-do list")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Open the terminal UI (default)
    Tui,
    /// Track a playlist as a course
    Add {
        /// Playlist URL
        url: String,
        /// Course name (defaults to the playlist title)
        #[arg(long)]
        name: Option<String>,
        /// Videos you already watched, e.g. 1-12,15 or all
        #[arg(long)]
        watched: Option<String>,
        /// How long a video really takes you, as a multiple of its length (e.g. 2 for maths)
        #[arg(long)]
        factor: Option<f64>,
    },
    /// Set how long videos of a course really take you (e.g. `factor maths 2`)
    Factor {
        course: String,
        /// Multiple of the video length: 2 means a 30 min video takes 60 min of study
        factor: f64,
    },
    /// List a course's videos with their numbers and watched state
    Videos { course: String },
    /// Mark videos as already watched, e.g. `mark rust 1-12,15` (or `all`)
    Mark {
        course: String,
        /// Video numbers from `videos`: 1-5,8,10-12 or all
        spec: String,
        /// Mark them as not watched instead
        #[arg(long)]
        undo: bool,
    },
    /// Stop tracking a course (number from `list`, or part of its name)
    Remove { course: String },
    /// Show all courses with progress
    List,
    /// Re-fetch playlists to pick up new/removed videos (all courses if none given)
    Sync { course: Option<String> },
    /// Show or change settings
    Config {
        /// Hours you plan to study each day
        #[arg(long)]
        hours: Option<f64>,
        /// How videos from several courses are mixed
        #[arg(long, value_enum)]
        mode: Option<Mode>,
    },
    /// Print today's to-do list
    Today {
        /// Re-plan the unwatched part of today's list
        #[arg(long)]
        refresh: bool,
    },
    /// Mark items of today's list as watched (by their number)
    Done {
        #[arg(required = true)]
        numbers: Vec<usize>,
    },
    /// Undo `done` for items of today's list
    Undone {
        #[arg(required = true)]
        numbers: Vec<usize>,
    },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let mut state = storage::load()?;
    state.ensure_today();

    match cli.cmd.unwrap_or(Cmd::Tui) {
        Cmd::Tui => {
            storage::save(&state)?;
            tui::run(&mut state)?;
            return Ok(());
        }
        Cmd::Add {
            url,
            name,
            watched,
            factor,
        } => {
            let fetched = youtube::fetch_playlist(&url)?;
            let skipped = fetched.skipped;
            let id = fetched.id.clone();
            let m = state.upsert_course(fetched, &url, name);
            if let Some(f) = factor {
                let ci = state
                    .courses
                    .iter()
                    .position(|c| c.id == id)
                    .expect("course was just added");
                state.set_time_factor(ci, f)?;
            }
            if let Some(spec) = watched {
                let ci = state
                    .courses
                    .iter()
                    .position(|c| c.id == id)
                    .expect("course was just added");
                let idx = model::parse_selection(&spec, state.courses[ci].videos.len())?;
                state.set_videos_watched(ci, &idx, true);
                println!("Marked {} video(s) as already watched.", idx.len());
            }
            state.refresh_today();
            let c = state.courses.last().filter(|_| m.is_new);
            if let Some(c) = c {
                println!(
                    "Added \"{}\": {} videos, {}",
                    c.name,
                    c.videos.len(),
                    fmt::hm(c.total_secs())
                );
            } else {
                println!("Already tracked; updated (+{} / -{} videos).", m.added, m.removed);
            }
            if skipped > 0 {
                println!("Skipped {skipped} private/deleted/live entries without a length.");
            }
        }
        Cmd::Remove { course } => {
            let i = find_course(&state, &course)?;
            let c = state.courses.remove(i);
            state.refresh_today();
            println!("Removed \"{}\".", c.name);
        }
        Cmd::List => {
            print_courses(&state);
            return Ok(());
        }
        Cmd::Factor { course, factor } => {
            let ci = find_course(&state, &course)?;
            state.set_time_factor(ci, factor)?;
            let c = &state.courses[ci];
            println!(
                "{}: ×{} study time. A 30:00 video now takes {} of your day.",
                c.name,
                c.time_factor,
                fmt::hm(c.study_secs(1800))
            );
        }
        Cmd::Videos { course } => {
            let c = &state.courses[find_course(&state, &course)?];
            println!(
                "{} · {}/{} watched · {}",
                c.name,
                c.watched_count(),
                c.videos.len(),
                fmt::bar(c.percent(), 20)
            );
            for (n, v) in c.videos.iter().enumerate() {
                println!(
                    "{:>3}. [{}] {} ({})",
                    n + 1,
                    if v.watched { "x" } else { " " },
                    v.title,
                    fmt::dur(v.duration_secs)
                );
            }
            return Ok(());
        }
        Cmd::Mark { course, spec, undo } => {
            let ci = find_course(&state, &course)?;
            let idx = model::parse_selection(&spec, state.courses[ci].videos.len())?;
            state.set_videos_watched(ci, &idx, !undo);
            let c = &state.courses[ci];
            println!(
                "{} video(s) marked as {}. {}: {}/{} watched · {}",
                idx.len(),
                if undo { "not watched" } else { "watched" },
                c.name,
                c.watched_count(),
                c.videos.len(),
                fmt::bar(c.percent(), 20)
            );
        }
        Cmd::Sync { course } => {
            let targets: Vec<usize> = match course {
                Some(sel) => vec![find_course(&state, &sel)?],
                None => (0..state.courses.len()).collect(),
            };
            let mut failed = 0;
            for i in targets {
                let (name, url) = (state.courses[i].name.clone(), state.courses[i].url.clone());
                match youtube::fetch_playlist(&url) {
                    Ok(f) => {
                        let m = state.upsert_course(f, &url, None);
                        println!("{name}: +{} new, -{} removed", m.added, m.removed);
                    }
                    Err(e) => {
                        failed += 1;
                        eprintln!("{name}: sync failed: {e:#}");
                    }
                }
            }
            state.refresh_today();
            storage::save(&state)?;
            if failed > 0 {
                bail!("{failed} course(s) failed to sync");
            }
            return Ok(());
        }
        Cmd::Config { hours, mode } => {
            if let Some(h) = hours {
                state.set_daily_hours(h)?;
            }
            if let Some(m) = mode {
                state.mode = m;
            }
            if hours.is_some() || mode.is_some() {
                state.refresh_today();
            }
            println!(
                "daily hours: {}\nmode: {:?}\ndata file: {}",
                state.daily_hours,
                state.mode,
                storage::path()?.display()
            );
        }
        Cmd::Today { refresh } => {
            if refresh {
                state.refresh_today();
            }
            print_today(&state);
        }
        Cmd::Done { numbers } => {
            for n in numbers {
                state.set_watched_today(n.saturating_sub(1), true)?;
            }
            print_today(&state);
        }
        Cmd::Undone { numbers } => {
            for n in numbers {
                state.set_watched_today(n.saturating_sub(1), false)?;
            }
            print_today(&state);
        }
    }
    storage::save(&state)
}

fn find_course(state: &State, sel: &str) -> Result<usize> {
    if let Ok(n) = sel.parse::<usize>() {
        if (1..=state.courses.len()).contains(&n) {
            return Ok(n - 1);
        }
    }
    let needle = sel.to_lowercase();
    let hits: Vec<usize> = state
        .courses
        .iter()
        .enumerate()
        .filter(|(_, c)| c.name.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect();
    match hits.as_slice() {
        [i] => Ok(*i),
        [] => bail!("no course matches '{sel}'"),
        _ => bail!("'{sel}' matches several courses; use its number from `studyplan list`"),
    }
}

fn print_courses(state: &State) {
    if state.courses.is_empty() {
        println!("No courses yet. Add one with: studyplan add <playlist-url>");
        return;
    }
    for (i, c) in state.courses.iter().enumerate() {
        let tag = if (c.time_factor - 1.0).abs() < 1e-9 {
            String::new()
        } else {
            format!("  (×{} study time)", c.time_factor)
        };
        println!(
            "{:>2}. {}{}\n    {}/{} videos · {} of {} · {}",
            i + 1,
            c.name,
            tag,
            c.watched_count(),
            c.videos.len(),
            fmt::hm(c.watched_secs()),
            fmt::hm(c.total_secs()),
            fmt::bar(c.percent(), 20)
        );
    }
    if let Some(days) = state.eta_days() {
        let finish = Local::now().date_naive() + Duration::days(days as i64 - 1);
        println!(
            "\nAt {:.1}h/day everything is done in about {days} day(s) (around {finish}).",
            state.daily_hours
        );
    }
}

fn print_today(state: &State) {
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
    println!(
        "Today ({}) · budget {:.1}h · planned {} · done {}",
        Local::now().format("%a %Y-%m-%d"),
        state.daily_hours,
        fmt::hm(planned),
        fmt::hm(done)
    );
    if items.is_empty() {
        println!("Nothing to watch. Add a course with: studyplan add <playlist-url>");
    }
    for (n, (c, v)) in items.iter().enumerate() {
        println!(
            "{:>2}. [{}] {} · {} ({})",
            n + 1,
            if v.watched { "x" } else { " " },
            c.name,
            v.title,
            c.label(v)
        );
    }
}
