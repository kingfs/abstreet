// Copyright 2018 Google LLC, licensed under http://www.apache.org/licenses/LICENSE-2.0

use colors::Colors;
use control::ControlMap;
use dimensioned::si;
use ezgui::GfxCtx;
use geom::{Bounds, Circle, Line, Polygon, Pt2D};
use map_model;
use map_model::{geometry, LaneID};
use objects::{Ctx, ID};
use render::{RenderOptions, Renderable, PARCEL_BOUNDARY_THICKNESS};

const MIN_ZOOM_FOR_LANE_MARKERS: f64 = 5.0;

#[derive(Debug)]
struct Marking {
    lines: Vec<Line>,
    color: Colors,
    thickness: f64,
    round: bool,
}

#[derive(Debug)]
pub struct DrawLane {
    pub id: LaneID,
    pub polygon: Polygon,
    start_crossing: Line,
    end_crossing: Line,
    markings: Vec<Marking>,

    // TODO pretty temporary
    draw_id_at: Vec<Pt2D>,
}

impl DrawLane {
    pub fn new(lane: &map_model::Lane, map: &map_model::Map, control_map: &ControlMap) -> DrawLane {
        let road = map.get_r(lane.parent);
        let start = new_perp_line(lane.first_line(), geometry::LANE_THICKNESS);
        let end = new_perp_line(lane.last_line().reverse(), geometry::LANE_THICKNESS);
        let polygon = lane
            .lane_center_pts
            .make_polygons_blindly(geometry::LANE_THICKNESS);

        let mut markings: Vec<Marking> = Vec::new();
        if road.is_canonical_lane(lane.id) {
            markings.push(Marking {
                lines: road.center_pts.lines(),
                color: Colors::RoadOrientation,
                thickness: geometry::BIG_ARROW_THICKNESS,
                round: true,
            });
        }
        match lane.lane_type {
            map_model::LaneType::Sidewalk => {
                markings.push(calculate_sidewalk_lines(lane));
            }
            map_model::LaneType::Parking => {
                markings.push(calculate_parking_lines(lane));
            }
            map_model::LaneType::Driving => {
                for m in calculate_driving_lines(lane, road) {
                    markings.push(m);
                }
            }
            map_model::LaneType::Biking => {}
        };
        if lane.is_driving() && !map.get_i(lane.dst_i).has_traffic_signal {
            if let Some(m) = calculate_stop_sign_line(lane, control_map) {
                markings.push(m);
            }
        }

        DrawLane {
            id: lane.id,
            polygon,
            markings,
            start_crossing: start,
            end_crossing: end,
            draw_id_at: calculate_id_positions(lane).unwrap_or(Vec::new()),
        }
    }

    fn draw_debug(&self, g: &mut GfxCtx, ctx: Ctx) {
        let circle_color = ctx.cs.get(Colors::BrightDebug);

        for l in ctx.map.get_l(self.id).lane_center_pts.lines() {
            g.draw_line(
                ctx.cs.get(Colors::Debug),
                PARCEL_BOUNDARY_THICKNESS / 2.0,
                &l,
            );
            g.draw_circle(circle_color, &Circle::new(l.pt1(), 0.4));
            g.draw_circle(circle_color, &Circle::new(l.pt2(), 0.8));
        }

        for pt in &self.draw_id_at {
            ctx.canvas
                .draw_text_at(g, &vec![format!("{}", self.id.0)], pt.x(), pt.y());
        }
    }

    // Get the line marking the end of the lane, perpendicular to the direction of the lane
    pub fn get_end_crossing(&self) -> &Line {
        &self.end_crossing
    }

    pub fn get_start_crossing(&self) -> &Line {
        &self.start_crossing
    }
}

impl Renderable for DrawLane {
    fn get_id(&self) -> ID {
        ID::Lane(self.id)
    }

    fn draw(&self, g: &mut GfxCtx, opts: RenderOptions, ctx: Ctx) {
        let color = opts.color.unwrap_or_else(|| {
            let l = ctx.map.get_l(self.id);
            let mut default = match l.lane_type {
                map_model::LaneType::Driving => ctx.cs.get(Colors::Road),
                map_model::LaneType::Parking => ctx.cs.get(Colors::Parking),
                map_model::LaneType::Sidewalk => ctx.cs.get(Colors::Sidewalk),
                map_model::LaneType::Biking => ctx.cs.get(Colors::Biking),
            };
            if l.probably_broken {
                default = ctx.cs.get(Colors::Broken);
            }
            default
        });
        g.draw_polygon(color, &self.polygon);

        if opts.cam_zoom >= MIN_ZOOM_FOR_LANE_MARKERS {
            for m in &self.markings {
                for line in &m.lines {
                    if m.round {
                        g.draw_rounded_line(ctx.cs.get(m.color), m.thickness, line);
                    } else {
                        g.draw_line(ctx.cs.get(m.color), m.thickness, line);
                    }
                }
            }
        }

        if opts.debug_mode {
            self.draw_debug(g, ctx);
        }
    }

    fn get_bounds(&self) -> Bounds {
        self.polygon.get_bounds()
    }

    fn contains_pt(&self, pt: Pt2D) -> bool {
        self.polygon.contains_pt(pt)
    }

    fn tooltip_lines(&self, map: &map_model::Map) -> Vec<String> {
        let l = map.get_l(self.id);
        let r = map.get_r(l.parent);
        let mut lines = vec![
            format!(
                "{} is {}",
                l.id,
                r.osm_tags.get("name").unwrap_or(&"???".to_string())
            ),
            format!("From OSM way {}, parent is {}", r.osm_way_id, r.id,),
            format!(
                "Lane goes from {} to {}",
                map.get_source_intersection(self.id).elevation,
                map.get_destination_intersection(self.id).elevation,
            ),
            format!("Lane is {}m long", l.length()),
        ];
        for (k, v) in &r.osm_tags {
            lines.push(format!("{} = {}", k, v));
        }
        lines
    }
}

// TODO this always does it at pt1
fn perp_line(l: Line, length: f64) -> Line {
    let pt1 = l.shift(length / 2.0).pt1();
    let pt2 = l.reverse().shift(length / 2.0).pt2();
    Line::new(pt1, pt2)
}

fn new_perp_line(l: Line, length: f64) -> Line {
    let pt1 = l.shift(length / 2.0).pt1();
    let pt2 = l.reverse().shift(length / 2.0).pt2();
    Line::new(pt1, pt2)
}

fn calculate_sidewalk_lines(lane: &map_model::Lane) -> Marking {
    let tile_every = geometry::LANE_THICKNESS * si::M;

    let length = lane.length();

    let mut lines = Vec::new();
    // Start away from the intersections
    let mut dist_along = tile_every;
    while dist_along < length - tile_every {
        let (pt, angle) = lane.dist_along(dist_along);
        // Reuse perp_line. Project away an arbitrary amount
        let pt2 = pt.project_away(1.0, angle);
        lines.push(perp_line(Line::new(pt, pt2), geometry::LANE_THICKNESS));
        dist_along += tile_every;
    }

    Marking {
        lines,
        color: Colors::SidewalkMarking,
        thickness: 0.25,
        round: false,
    }
}

fn calculate_parking_lines(lane: &map_model::Lane) -> Marking {
    // meters, but the dims get annoying below to remove
    // TODO make Pt2D natively understand meters, projecting away by an angle
    let leg_length = 1.0;

    let mut lines = Vec::new();
    let num_spots = lane.number_parking_spots();
    if num_spots > 0 {
        for idx in 0..=num_spots {
            let (pt, lane_angle) =
                lane.dist_along(map_model::PARKING_SPOT_LENGTH * (1.0 + idx as f64));
            let perp_angle = lane_angle.rotate_degs(270.0);
            // Find the outside of the lane. Actually, shift inside a little bit, since the line will
            // have thickness, but shouldn't really intersect the adjacent line when drawn.
            let t_pt = pt.project_away(geometry::LANE_THICKNESS * 0.4, perp_angle);
            // The perp leg
            let p1 = t_pt.project_away(leg_length, perp_angle.opposite());
            lines.push(Line::new(t_pt, p1));
            // Upper leg
            let p2 = t_pt.project_away(leg_length, lane_angle);
            lines.push(Line::new(t_pt, p2));
            // Lower leg
            let p3 = t_pt.project_away(leg_length, lane_angle.opposite());
            lines.push(Line::new(t_pt, p3));
        }
    }

    Marking {
        lines,
        color: Colors::ParkingMarking,
        thickness: 0.25,
        round: false,
    }
}

fn calculate_driving_lines(lane: &map_model::Lane, parent: &map_model::Road) -> Option<Marking> {
    // The rightmost lanes don't have dashed white lines.
    if parent.dir_and_offset(lane.id).1 == 0 {
        return None;
    }

    // Project left, so reverse the points.
    let center_pts = lane.lane_center_pts.reversed();
    let lane_edge_pts = center_pts.shift_blindly(geometry::LANE_THICKNESS / 2.0);

    // This is an incredibly expensive way to compute dashed polyines, and it doesn't follow bends
    // properly. Just a placeholder.
    let lane_len = lane_edge_pts.length();
    let dash_separation = 2.0 * si::M;
    let dash_len = 1.0 * si::M;

    let mut lines = Vec::new();
    let mut start = dash_separation;
    loop {
        if start + dash_len >= lane_len - dash_separation {
            break;
        }

        let (pt1, _) = lane_edge_pts.dist_along(start);
        let (pt2, _) = lane_edge_pts.dist_along(start + dash_len);
        lines.push(Line::new(pt1, pt2));
        start += dash_len + dash_separation;
    }

    Some(Marking {
        lines,
        color: Colors::DrivingLaneMarking,
        thickness: 0.25,
        round: false,
    })
}

fn calculate_stop_sign_line(lane: &map_model::Lane, control_map: &ControlMap) -> Option<Marking> {
    if control_map.stop_signs[&lane.dst_i].is_priority_lane(lane.id) {
        return None;
    }

    // TODO maybe draw the stop sign octagon on each lane?

    let (pt1, angle) =
        lane.safe_dist_along(lane.length() - (2.0 * geometry::LANE_THICKNESS * si::M))?;
    // Reuse perp_line. Project away an arbitrary amount
    let pt2 = pt1.project_away(1.0, angle);
    Some(Marking {
        lines: vec![perp_line(Line::new(pt1, pt2), geometry::LANE_THICKNESS)],
        color: Colors::StopSignMarking,
        thickness: 0.45,
        round: true,
    })
}

fn calculate_id_positions(lane: &map_model::Lane) -> Option<Vec<Pt2D>> {
    if !lane.is_driving() {
        return None;
    }

    let (pt1, _) =
        lane.safe_dist_along(lane.length() - (2.0 * geometry::LANE_THICKNESS * si::M))?;
    let (pt2, _) = lane.safe_dist_along(2.0 * geometry::LANE_THICKNESS * si::M)?;
    Some(vec![pt1, pt2])
}
