//! Decides which videos fit into today's study budget.

use std::collections::VecDeque;

use crate::model::{Course, Mode, TodoItem, Video};

/// Build today's to-do list.
///
/// * `keep` – items that stay on the list no matter what (videos already
///   watched today). They count against the budget.
/// * Videos are always taken in playlist order within a course.
/// * A video is added only if it fits in the remaining time. The one
///   exception: if nothing is planned yet, the next video is added even when
///   it is longer than the whole budget, so a long lecture can never block a
///   course forever.
pub fn build(courses: &[Course], mode: Mode, budget: u64, keep: Vec<TodoItem>) -> Vec<TodoItem> {
    let duration_of = |it: &TodoItem| -> Option<u64> {
        let c = courses.iter().find(|c| c.id == it.course_id)?;
        let v = c.videos.iter().find(|v| v.id == it.video_id)?;
        Some(c.study_secs(v.duration_secs))
    };

    let mut items: Vec<TodoItem> = keep
        .into_iter()
        .filter(|i| duration_of(i).is_some())
        .collect();
    let mut used: u64 = items.iter().filter_map(duration_of).sum();

    let mut queues: Vec<VecDeque<(&Course, &Video)>> = courses
        .iter()
        .map(|c| {
            c.videos
                .iter()
                .filter(|v| {
                    !v.watched
                        && !items
                            .iter()
                            .any(|i| i.course_id == c.id && i.video_id == v.id)
                })
                .map(|v| (c, v))
                .collect()
        })
        .collect();

    let fits = |used: u64, d: u64| used == 0 || used + d <= budget;

    // Interleave order: least-finished course first (ties keep the order the
    // courses were added in). Without this, a small budget would always be
    // used up by the first courses and the last ones would never get a turn.
    let mut order: Vec<usize> = (0..courses.len()).collect();
    order.sort_by(|&a, &b| {
        courses[a]
            .percent()
            .total_cmp(&courses[b].percent())
            .then(a.cmp(&b))
    });

    match mode {
        // Course 1 first, then whatever time is left goes to course 2, ...
        Mode::Sequential => {
            for q in queues.iter_mut() {
                while let Some(&(c, v)) = q.front() {
                    if !fits(used, c.study_secs(v.duration_secs)) {
                        break;
                    }
                    used += c.study_secs(v.duration_secs);
                    items.push(TodoItem {
                        course_id: c.id.clone(),
                        video_id: v.id.clone(),
                    });
                    q.pop_front();
                }
            }
        }
        // One video from each course in turn until nothing else fits.
        Mode::Interleave => {
            let mut progressed = true;
            while progressed {
                progressed = false;
                for &i in &order {
                    let q = &mut queues[i];
                    if let Some(&(c, v)) = q.front() {
                        if fits(used, c.study_secs(v.duration_secs)) {
                            used += c.study_secs(v.duration_secs);
                            items.push(TodoItem {
                                course_id: c.id.clone(),
                                video_id: v.id.clone(),
                            });
                            q.pop_front();
                            progressed = true;
                        }
                    }
                }
            }
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn course(id: &str, mins: &[u64]) -> Course {
        Course {
            id: id.into(),
            name: id.into(),
            url: String::new(),
            time_factor: 1.0,
            videos: mins
                .iter()
                .enumerate()
                .map(|(i, m)| Video {
                    id: format!("{id}{i}"),
                    title: format!("{id} video {i}"),
                    duration_secs: m * 60,
                    watched: false,
                })
                .collect(),
        }
    }

    fn ids(items: &[TodoItem]) -> Vec<String> {
        items.iter().map(|i| i.video_id.clone()).collect()
    }

    #[test]
    fn sequential_fills_budget_in_order() {
        let cs = vec![course("a", &[30, 30, 30]), course("b", &[20, 20])];
        // 100 minute budget: a0 a1 a2 = 90, then b0 (20) does not fit.
        let plan = build(&cs, Mode::Sequential, 100 * 60, vec![]);
        assert_eq!(ids(&plan), ["a0", "a1", "a2"]);
    }

    #[test]
    fn sequential_leftover_goes_to_next_course() {
        let cs = vec![course("a", &[50, 50]), course("b", &[10, 10])];
        // a0 (50) fits, a1 (50) does not -> b0, b1 use the remaining 20.
        let plan = build(&cs, Mode::Sequential, 70 * 60, vec![]);
        assert_eq!(ids(&plan), ["a0", "b0", "b1"]);
    }

    #[test]
    fn interleave_alternates() {
        let cs = vec![course("a", &[10, 10, 10]), course("b", &[10, 10, 10])];
        let plan = build(&cs, Mode::Interleave, 40 * 60, vec![]);
        assert_eq!(ids(&plan), ["a0", "b0", "a1", "b1"]);
    }

    #[test]
    fn interleave_serves_least_finished_course_first() {
        let mut cs = vec![
            course("a", &[30, 30, 30]),
            course("b", &[30, 30, 30]),
            course("c", &[30, 30, 30]),
        ];
        cs[0].videos[0].watched = true;
        cs[1].videos[0].watched = true;
        // 60 min budget: c is at 0% so it goes first; a and b tie at 33%, a wins by order.
        // (Without this, c would starve whenever the budget only covers two videos.)
        let plan = build(&cs, Mode::Interleave, 60 * 60, vec![]);
        assert_eq!(ids(&plan), ["c0", "a1"]);
    }

    #[test]
    fn time_factor_scales_what_a_video_costs() {
        let mut cs = vec![course("m", &[30, 30, 30, 30])];
        cs[0].time_factor = 2.0; // each 30 min video takes 60 min of study
        // 150 min budget: two videos (120 min); a third (180 min) does not fit.
        let plan = build(&cs, Mode::Interleave, 150 * 60, vec![]);
        assert_eq!(ids(&plan), ["m0", "m1"]);
    }

    #[test]
    fn factors_are_applied_per_course() {
        let mut cs = vec![course("math", &[30, 30, 30]), course("hist", &[30, 30, 30])];
        cs[0].time_factor = 2.0; // math: 60 min each, history: 30 min each
        // 120 min: math0 (60) + hist0 (30) + hist1 (30); math1 would need 60 more.
        let plan = build(&cs, Mode::Interleave, 120 * 60, vec![]);
        assert_eq!(ids(&plan), ["math0", "hist0", "hist1"]);
    }

    #[test]
    fn oversized_video_is_still_planned_when_alone() {
        let cs = vec![course("a", &[180, 10])];
        let plan = build(&cs, Mode::Sequential, 60 * 60, vec![]);
        assert_eq!(ids(&plan), ["a0"]);
    }

    #[test]
    fn watched_items_are_kept_and_count_against_budget() {
        let mut cs = vec![course("a", &[30, 30, 30])];
        cs[0].videos[0].watched = true;
        let keep = vec![TodoItem {
            course_id: "a".into(),
            video_id: "a0".into(),
        }];
        // budget 60: a0 (done, 30) + a1 (30); a2 does not fit.
        let plan = build(&cs, Mode::Sequential, 60 * 60, keep);
        assert_eq!(ids(&plan), ["a0", "a1"]);
    }

    #[test]
    fn nothing_left_gives_empty_plan() {
        let mut cs = vec![course("a", &[10])];
        cs[0].videos[0].watched = true;
        assert!(build(&cs, Mode::Sequential, 3600, vec![]).is_empty());
    }
}
