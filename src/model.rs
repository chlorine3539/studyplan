//! Core data types and the operations on them.

use std::collections::HashSet;

use anyhow::{Result, bail};
use chrono::{Local, NaiveDate};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::{planner, youtube::Fetched};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Finish courses in the order they were added; leftover time goes to the next one
    Sequential,
    /// Take one video from each course in turn, least-finished course first
    #[default]
    Interleave,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub duration_secs: u64,
    #[serde(default)]
    pub watched: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Course {
    /// YouTube playlist id
    pub id: String,
    pub name: String,
    pub url: String,
    pub videos: Vec<Video>,
    /// How long a video of this course really takes you, as a multiple of its
    /// length (e.g. 2.0 for maths: pausing, solving, taking notes).
    #[serde(default = "one")]
    pub time_factor: f64,
}

fn one() -> f64 {
    1.0
}

impl Course {
    /// Study time (seconds of your day) that `video_secs` of video costs.
    pub fn study_secs(&self, video_secs: u64) -> u64 {
        (video_secs as f64 * self.time_factor).round() as u64
    }

    /// Video length, followed by the real study time when the factor isn't 1.
    pub fn label(&self, v: &Video) -> String {
        let len = crate::fmt::dur(v.duration_secs);
        if (self.time_factor - 1.0).abs() < 1e-9 {
            len
        } else {
            format!("{len} → {}", crate::fmt::hm(self.study_secs(v.duration_secs)))
        }
    }

    pub fn total_secs(&self) -> u64 {
        self.videos.iter().map(|v| v.duration_secs).sum()
    }
    pub fn watched_secs(&self) -> u64 {
        self.videos
            .iter()
            .filter(|v| v.watched)
            .map(|v| v.duration_secs)
            .sum()
    }
    pub fn watched_count(&self) -> usize {
        self.videos.iter().filter(|v| v.watched).count()
    }
    /// Completion by watch time (a 1-hour lecture counts more than a 2-minute one).
    pub fn percent(&self) -> f64 {
        let total = self.total_secs();
        if total == 0 {
            0.0
        } else {
            self.watched_secs() as f64 * 100.0 / total as f64
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub course_id: String,
    pub video_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayPlan {
    pub date: NaiveDate,
    pub items: Vec<TodoItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub daily_hours: f64,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub courses: Vec<Course>,
    #[serde(default)]
    pub today: Option<DayPlan>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            daily_hours: 2.0,
            mode: Mode::default(),
            courses: Vec::new(),
            today: None,
        }
    }
}

pub struct Merge {
    pub added: usize,
    pub removed: usize,
    pub is_new: bool,
}

impl State {
    pub fn budget_secs(&self) -> u64 {
        (self.daily_hours * 3600.0).round() as u64
    }

    pub fn set_daily_hours(&mut self, hours: f64) -> Result<()> {
        if !(0.25..=16.0).contains(&hours) {
            bail!("daily hours must be between 0.25 and 16");
        }
        self.daily_hours = hours;
        Ok(())
    }

    /// Set how long videos of a course really take you, as a multiple of their
    /// length, and re-plan today (2.0 means a 30 min video costs 60 min).
    pub fn set_time_factor(&mut self, course: usize, factor: f64) -> Result<()> {
        if !(0.25..=10.0).contains(&factor) {
            bail!("the factor must be between 0.25 and 10");
        }
        let c = self
            .courses
            .get_mut(course)
            .ok_or_else(|| anyhow::anyhow!("no such course"))?;
        c.time_factor = factor;
        self.refresh_today();
        Ok(())
    }

    fn is_watched(&self, it: &TodoItem) -> bool {
        self.find(it).is_some_and(|(_, v)| v.watched)
    }

    pub fn find(&self, it: &TodoItem) -> Option<(&Course, &Video)> {
        let c = self.courses.iter().find(|c| c.id == it.course_id)?;
        let v = c.videos.iter().find(|v| v.id == it.video_id)?;
        Some((c, v))
    }

    /// Today's to-do list, resolved to course/video references.
    pub fn today_items(&self) -> Vec<(&Course, &Video)> {
        self.today
            .as_ref()
            .map(|p| p.items.iter().filter_map(|i| self.find(i)).collect())
            .unwrap_or_default()
    }

    /// Make sure there is a plan for the current date (builds a fresh one
    /// after midnight; yesterday's unwatched videos simply come up again).
    pub fn ensure_today(&mut self) {
        let today = Local::now().date_naive();
        if self.today.as_ref().map(|p| p.date) != Some(today) {
            self.today = None;
            self.refresh_today();
        }
    }

    /// Re-plan today: videos already watched today stay, everything else is
    /// recomputed against the current budget, mode and course list.
    pub fn refresh_today(&mut self) {
        let keep: Vec<TodoItem> = self
            .today
            .as_ref()
            .map(|p| {
                p.items
                    .iter()
                    .filter(|i| self.is_watched(i))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let items = planner::build(&self.courses, self.mode, self.budget_secs(), keep);
        self.today = Some(DayPlan {
            date: Local::now().date_naive(),
            items,
        });
    }

    /// Mark the `idx`-th (0-based) item of today's list as watched/unwatched.
    pub fn set_watched_today(&mut self, idx: usize, watched: bool) -> Result<()> {
        let item = self
            .today_items()
            .get(idx)
            .map(|(c, v)| TodoItem {
                course_id: c.id.clone(),
                video_id: v.id.clone(),
            })
            .ok_or_else(|| anyhow::anyhow!("there is no item #{} in today's list", idx + 1))?;
        self.set_watched(&item, watched);
        Ok(())
    }

    pub fn toggle_today(&mut self, idx: usize) -> Result<()> {
        let now = self
            .today_items()
            .get(idx)
            .map(|(_, v)| v.watched)
            .ok_or_else(|| anyhow::anyhow!("there is no item #{} in today's list", idx + 1))?;
        self.set_watched_today(idx, !now)
    }

    /// Mark videos of a course (0-based positions in its playlist) as watched or
    /// not watched, e.g. for videos seen before you started using this tool.
    ///
    /// These count as "watched earlier", not as progress on today's budget, so
    /// any of them sitting on today's list are dropped and the list is re-planned.
    /// (To count a video toward today, tick it on the Today list instead.)
    pub fn set_videos_watched(&mut self, course: usize, idxs: &[usize], watched: bool) {
        let Some(c) = self.courses.get_mut(course) else {
            return;
        };
        let mut touched = HashSet::new();
        for &i in idxs {
            if let Some(v) = c.videos.get_mut(i) {
                v.watched = watched;
                touched.insert(v.id.clone());
            }
        }
        let course_id = c.id.clone();
        if let Some(plan) = self.today.as_mut() {
            plan.items
                .retain(|it| !(it.course_id == course_id && touched.contains(&it.video_id)));
        }
        self.refresh_today();
    }

    fn set_watched(&mut self, it: &TodoItem, watched: bool) {
        if let Some(v) = self
            .courses
            .iter_mut()
            .find(|c| c.id == it.course_id)
            .and_then(|c| c.videos.iter_mut().find(|v| v.id == it.video_id))
        {
            v.watched = watched;
        }
    }

    /// Insert a freshly fetched playlist, or update it if it is already tracked
    /// (watched flags are preserved by video id).
    pub fn upsert_course(&mut self, f: Fetched, url: &str, name: Option<String>) -> Merge {
        if let Some(c) = self.courses.iter_mut().find(|c| c.id == f.id) {
            let old_ids: HashSet<String> = c.videos.iter().map(|v| v.id.clone()).collect();
            let watched: HashSet<String> = c
                .videos
                .iter()
                .filter(|v| v.watched)
                .map(|v| v.id.clone())
                .collect();
            let new_ids: HashSet<String> = f.videos.iter().map(|v| v.id.clone()).collect();
            let added = new_ids.difference(&old_ids).count();
            let removed = old_ids.difference(&new_ids).count();

            c.videos = f
                .videos
                .into_iter()
                .map(|mut v| {
                    v.watched = watched.contains(&v.id);
                    v
                })
                .collect();
            c.url = url.to_string();
            if let Some(n) = name {
                c.name = n;
            }
            Merge {
                added,
                removed,
                is_new: false,
            }
        } else {
            let added = f.videos.len();
            self.courses.push(Course {
                id: f.id,
                name: name.unwrap_or(f.title),
                url: url.to_string(),
                videos: f.videos,
                time_factor: 1.0,
            });
            Merge {
                added,
                removed: 0,
                is_new: true,
            }
        }
    }

    /// Estimated days to finish everything at the current daily budget.
    pub fn eta_days(&self) -> Option<u64> {
        let remaining: u64 = self
            .courses
            .iter()
            .map(|c| c.study_secs(c.total_secs() - c.watched_secs()))
            .sum();
        let budget = self.budget_secs();
        if remaining == 0 || budget == 0 {
            None
        } else {
            Some(remaining.div_ceil(budget))
        }
    }
}

/// Parse a video selection like `1-5,8,10-12` or `all` (1-based, as shown by
/// `studyplan videos`) into sorted, unique 0-based indexes.
pub fn parse_selection(spec: &str, len: usize) -> Result<Vec<usize>> {
    let spec = spec.trim();
    if spec.eq_ignore_ascii_case("all") {
        return Ok((0..len).collect());
    }
    let mut out = std::collections::BTreeSet::new();
    for part in spec.split(',') {
        let part = part.trim();
        let (a, b) = part.split_once('-').unwrap_or((part, part));
        let (Ok(a), Ok(b)) = (a.trim().parse::<usize>(), b.trim().parse::<usize>()) else {
            bail!("'{part}' is not a video number or range (use e.g. 1-5,8,10-12 or all)");
        };
        if a == 0 || b < a || b > len {
            bail!("'{part}' is out of range: this course has videos 1-{len}");
        }
        out.extend(a - 1..b);
    }
    Ok(out.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_parsing() {
        assert_eq!(parse_selection("1-3,5", 10).unwrap(), [0, 1, 2, 4]);
        assert_eq!(parse_selection(" 2 , 2-3 ", 10).unwrap(), [1, 2]);
        assert_eq!(parse_selection("ALL", 3).unwrap(), [0, 1, 2]);
        for bad in ["", "0", "3-2", "11", "1-", "x", "1,,2"] {
            assert!(parse_selection(bad, 10).is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn marking_earlier_videos_replans_without_counting_them_as_today() {
        let vid = |id: &str, m: u64| Video {
            id: id.into(),
            title: id.into(),
            duration_secs: m * 60,
            watched: false,
        };
        let mut s = State::default();
        s.daily_hours = 1.0;
        s.courses = vec![Course {
            id: "a".into(),
            name: "A".into(),
            url: String::new(),
            videos: vec![vid("a0", 30), vid("a1", 30), vid("a2", 30)],
            time_factor: 1.0,
        }];
        s.refresh_today();
        let titles = |s: &State| -> Vec<String> {
            s.today_items().iter().map(|(_, v)| v.title.clone()).collect()
        };
        assert_eq!(titles(&s), ["a0", "a1"]);

        // "I already watched a0 and a1 before": they leave the list, a2 moves up,
        // and neither counts as done today.
        s.set_videos_watched(0, &[0, 1], true);
        assert_eq!(titles(&s), ["a2"]);
        assert_eq!(s.courses[0].watched_count(), 2);

        // Undoing brings them back.
        s.set_videos_watched(0, &[0], false);
        assert_eq!(titles(&s), ["a0", "a2"]);
    }

    #[test]
    fn time_factor_changes_what_fits_and_is_validated() {
        let mut s = State::default();
        s.daily_hours = 1.0;
        s.courses = vec![Course {
            id: "m".into(),
            name: "Math".into(),
            url: String::new(),
            videos: (0..3)
                .map(|i| Video {
                    id: format!("m{i}"),
                    title: format!("m{i}"),
                    duration_secs: 1800,
                    watched: false,
                })
                .collect(),
            time_factor: 1.0,
        }];
        s.refresh_today();
        assert_eq!(s.today_items().len(), 2); // 2 x 30 min fit in 1 h

        s.set_time_factor(0, 2.0).unwrap();
        assert_eq!(s.today_items().len(), 1); // 1 x (30 min x 2)
        assert_eq!(s.courses[0].study_secs(1800), 3600);
        assert_eq!(s.eta_days(), Some(3)); // 3 x 60 min of study at 1 h/day

        assert!(s.set_time_factor(0, 0.0).is_err());
        assert!(s.set_time_factor(0, f64::NAN).is_err());
        assert!(s.set_time_factor(5, 1.0).is_err());
    }

    #[test]
    fn old_state_files_without_a_factor_still_load() {
        let c: Course =
            serde_json::from_str(r#"{"id":"a","name":"A","url":"","videos":[]}"#).unwrap();
        assert_eq!(c.time_factor, 1.0);
    }
}
