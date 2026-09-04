use crate::style::{Style, StyledSpan};
use crate::theme::Theme;
use crossterm::style::Color;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

// ───── Data types ─────

#[derive(Debug, Clone, Copy, PartialEq)]
enum Direction {
    TopDown,
    LeftRight,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum NodeShape {
    Rectangle,
    Rounded,
    Diamond,
    Circle,
}

/// A row to be drawn inside a multi-row card node.
pub(crate) struct CardDrawRow {
    pub key: String,
    pub value_text: String,
    pub value_color: Option<Color>,
    /// If true, the value area shows `──▶` instead of text.
    pub is_connector: bool,
}

#[derive(Debug, Clone)]
struct Node {
    label: String,
    shape: NodeShape,
}

#[derive(Debug, Clone)]
struct Edge {
    from: String,
    to: String,
    label: Option<String>,
}

#[derive(Debug)]
struct Graph {
    direction: Direction,
    nodes: HashMap<String, Node>,
    edges: Vec<Edge>,
    node_order: Vec<String>,
}

// ───── Parser ─────

fn parse_mermaid(code: &str) -> Option<Graph> {
    let mut direction = Direction::TopDown;
    let mut nodes: HashMap<String, Node> = HashMap::new();
    let mut edges: Vec<Edge> = Vec::new();
    let mut node_order: Vec<String> = Vec::new();

    for line in code.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() || trimmed.starts_with("%%") {
            continue;
        }

        // Direction declaration
        if trimmed.starts_with("graph ") || trimmed.starts_with("flowchart ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 {
                direction = match parts[1] {
                    "LR" | "RL" => Direction::LeftRight,
                    _ => Direction::TopDown,
                };
            }
            continue;
        }

        // Skip unsupported directives
        if trimmed.starts_with("subgraph")
            || trimmed == "end"
            || trimmed.starts_with("style ")
            || trimmed.starts_with("classDef ")
            || trimmed.starts_with("class ")
            || trimmed.starts_with("linkStyle ")
            || trimmed.starts_with("click ")
        {
            continue;
        }

        parse_line(trimmed, &mut nodes, &mut edges, &mut node_order);
    }

    if nodes.is_empty() {
        return None;
    }

    Some(Graph {
        direction,
        nodes,
        edges,
        node_order,
    })
}

#[allow(clippy::type_complexity)]
fn parse_node_ref(s: &str) -> Option<(String, Option<(String, NodeShape)>, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }

    // Extract node ID (alphanumeric, underscore, hyphen)
    let id_end = s
        .find(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .unwrap_or(s.len());
    if id_end == 0 {
        return None;
    }
    let id = s[..id_end].to_string();
    let rest = &s[id_end..];

    // Double parens: ((label))
    if rest.starts_with("((")
        && let Some(end) = rest.find("))")
    {
        let label = rest[2..end].trim().to_string();
        return Some((id, Some((label, NodeShape::Circle)), &rest[end + 2..]));
    }

    // Square brackets: [label]
    if rest.starts_with('[')
        && let Some(end) = find_matching(rest, '[', ']')
    {
        let label = rest[1..end].trim().to_string();
        return Some((id, Some((label, NodeShape::Rectangle)), &rest[end + 1..]));
    }

    // Curly braces: {label}
    if rest.starts_with('{')
        && let Some(end) = find_matching(rest, '{', '}')
    {
        let label = rest[1..end].trim().to_string();
        return Some((id, Some((label, NodeShape::Diamond)), &rest[end + 1..]));
    }

    // Parentheses: (label)
    if rest.starts_with('(')
        && let Some(end) = find_matching(rest, '(', ')')
    {
        let label = rest[1..end].trim().to_string();
        return Some((id, Some((label, NodeShape::Rounded)), &rest[end + 1..]));
    }

    Some((id, None, rest))
}

fn find_matching(s: &str, open: char, close: char) -> Option<usize> {
    let mut depth: usize = 0;
    for (i, c) in s.char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            if depth == 0 {
                continue;
            }
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

fn register_node(
    id: &str,
    label_shape: Option<(String, NodeShape)>,
    nodes: &mut HashMap<String, Node>,
    node_order: &mut Vec<String>,
) {
    if let Some(node) = nodes.get_mut(id) {
        if let Some((label, shape)) = label_shape {
            node.label = label;
            node.shape = shape;
        }
    } else {
        let (label, shape) = label_shape.unwrap_or_else(|| (id.to_string(), NodeShape::Rectangle));
        nodes.insert(id.to_string(), Node { label, shape });
        node_order.push(id.to_string());
    }
}

fn parse_line(
    line: &str,
    nodes: &mut HashMap<String, Node>,
    edges: &mut Vec<Edge>,
    node_order: &mut Vec<String>,
) {
    let (first_id, first_label, mut remaining) = match parse_node_ref(line) {
        Some(r) => r,
        None => return,
    };
    register_node(&first_id, first_label, nodes, node_order);

    let mut prev_id = first_id;

    // Parse chain of edges: A --> B --> C
    loop {
        let trimmed = remaining.trim_start();
        if trimmed.is_empty() {
            break;
        }

        let (edge_label, arrow_rest) = match parse_arrow(trimmed) {
            Some(r) => r,
            None => break,
        };

        remaining = arrow_rest;

        let (next_id, next_label, rest) = match parse_node_ref(remaining) {
            Some(r) => r,
            None => break,
        };
        register_node(&next_id, next_label, nodes, node_order);

        edges.push(Edge {
            from: prev_id.clone(),
            to: next_id.clone(),
            label: edge_label,
        });

        prev_id = next_id;
        remaining = rest;
    }
}

fn parse_arrow(s: &str) -> Option<(Option<String>, &str)> {
    let s = s.trim_start();

    // "-- label -->" syntax
    if s.starts_with("-- ")
        && let Some(arrow_pos) = s[3..].find("-->")
    {
        let label = s[3..3 + arrow_pos].trim().to_string();
        let rest = &s[3 + arrow_pos + 3..];
        return Some((Some(label), rest));
    }

    // Standard arrows
    let arrows = ["--->", "-->", "---", "-.->", "==>"];
    for arrow in &arrows {
        if let Some(rest) = s.strip_prefix(arrow) {
            // Check for |label| after arrow
            let trimmed_rest = rest.trim_start();
            if trimmed_rest.starts_with('|')
                && let Some(end) = trimmed_rest[1..].find('|')
            {
                let label = trimmed_rest[1..1 + end].trim().to_string();
                return Some((Some(label), &trimmed_rest[2 + end..]));
            }
            return Some((None, rest));
        }
    }

    None
}

// ───── Layout ─────

#[derive(Clone)]
pub(crate) struct NodeLayout {
    pub(crate) center_x: usize,
    pub(crate) top_y: usize,
    pub(crate) width: usize,
}

impl NodeLayout {
    /// Column of the left border character.
    fn left_x(&self) -> usize {
        self.center_x.saturating_sub(self.width / 2)
    }

    /// Column of the right border character.
    fn right_x(&self) -> usize {
        self.left_x() + self.width.saturating_sub(1)
    }

    /// Row of the bottom border character.
    fn bottom_y(&self) -> usize {
        self.top_y + 2
    }
}

/// Output of the layering phase, shared by both renderers.
struct Layout {
    /// Nodes grouped by layer; each layer is ordered left-to-right (TD) or
    /// top-to-bottom (LR).
    layers: Vec<Vec<String>>,
    /// `(layer index, position within layer)` for every node.
    node_pos: HashMap<String, (usize, usize)>,
    /// Indices into `graph.edges` of the feedback (back) edges, sorted by the
    /// number of layers they span, shortest first. Gutter lane `k` carries
    /// `feedback[k]`, so the shortest edge sits innermost and longer edges wrap
    /// around it instead of crossing it.
    feedback: Vec<usize>,
    /// The same indices as a set, for the renderers' "is this edge routed as
    /// feedback?" test.
    feedback_set: HashSet<usize>,
}

fn layout(graph: &Graph) -> Layout {
    let feedback_set = classify_feedback_edges(graph);
    let mut layers = assign_layers(graph, &feedback_set);
    order_within_layers(&mut layers, graph, &feedback_set);

    let mut node_pos: HashMap<String, (usize, usize)> = HashMap::new();
    for (layer_idx, layer) in layers.iter().enumerate() {
        for (pos, id) in layer.iter().enumerate() {
            node_pos.insert(id.clone(), (layer_idx, pos));
        }
    }

    let span = |idx: usize| {
        let edge = &graph.edges[idx];
        match (node_pos.get(&edge.from), node_pos.get(&edge.to)) {
            (Some(&(from, _)), Some(&(to, _))) => from.abs_diff(to),
            _ => 0,
        }
    };
    let mut feedback: Vec<usize> = feedback_set.iter().copied().collect();
    feedback.sort_by_key(|&idx| (span(idx), idx));

    Layout {
        layers,
        node_pos,
        feedback,
        feedback_set,
    }
}

/// Routing decisions for one feedback edge.
struct FeedbackPlan {
    /// Index into `graph.edges`.
    edge: usize,
    /// Gutter lane, 0 = innermost.
    lane: usize,
    /// Rank of the source among the feedback sources in its layer, counted
    /// from the gutter side (0 = nearest the gutter). Every rank gets its own
    /// row (TD) or gap column (LR), so routes that share a layer neither merge
    /// nor cross.
    src_rank: usize,
    /// Same for the destination among the feedback targets in its layer.
    dst_rank: usize,
}

/// Routing decisions for all the feedback edges of one diagram.
struct FeedbackPlans {
    plans: Vec<FeedbackPlan>,
    /// Number of distinct feedback sources in each layer. The gap after a
    /// layer has to hold one row (TD) or column (LR) per source.
    exits: Vec<usize>,
    /// Number of distinct feedback targets in each layer, likewise budgeted in
    /// the gap before it.
    entries: Vec<usize>,
}

fn plan_feedback(graph: &Graph, layout: &Layout) -> FeedbackPlans {
    let layer_count = layout.layers.len();
    let mut sources: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); layer_count];
    let mut targets: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); layer_count];
    for &idx in &layout.feedback {
        let edge = &graph.edges[idx];
        if let Some(&(layer, pos)) = layout.node_pos.get(&edge.from) {
            sources[layer].insert(pos);
        }
        if let Some(&(layer, pos)) = layout.node_pos.get(&edge.to) {
            targets[layer].insert(pos);
        }
    }
    // Rank counts the endpoints between this one and the gutter.
    let rank = |positions: &BTreeSet<usize>, pos: usize| positions.range(pos + 1..).count();

    let mut plans: Vec<FeedbackPlan> = layout
        .feedback
        .iter()
        .filter_map(|&idx| {
            let edge = &graph.edges[idx];
            let &(src_layer, src_pos) = layout.node_pos.get(&edge.from)?;
            let &(dst_layer, dst_pos) = layout.node_pos.get(&edge.to)?;
            Some(FeedbackPlan {
                edge: idx,
                lane: 0,
                src_rank: rank(&sources[src_layer], src_pos),
                dst_rank: rank(&targets[dst_layer], dst_pos),
            })
        })
        .collect();
    // Lanes are numbered over the edges that survived, so they stay contiguous.
    for (lane, plan) in plans.iter_mut().enumerate() {
        plan.lane = lane;
    }

    FeedbackPlans {
        plans,
        exits: sources.iter().map(BTreeSet::len).collect(),
        entries: targets.iter().map(BTreeSet::len).collect(),
    }
}

/// Geometry of one feedback edge route in a top-down diagram.
/// See [`Canvas::draw_feedback_edge_td`].
pub(crate) struct FeedbackRouteTd {
    /// Column where the route leaves the source's bottom border.
    pub(crate) exit_x: usize,
    /// Row of the source's bottom border.
    pub(crate) src_bottom_y: usize,
    /// Gap row of the horizontal run below the source.
    pub(crate) exit_y: usize,
    /// Column where the arrowhead enters the destination's top border.
    pub(crate) entry_x: usize,
    /// Row of the destination's top border.
    pub(crate) dst_top_y: usize,
    /// Gap row of the horizontal run above the destination.
    pub(crate) entry_y: usize,
    /// Column of the vertical lane in the gutter right of the diagram.
    pub(crate) lane_x: usize,
}

/// Geometry of one feedback edge route in a left-right diagram.
/// See [`Canvas::draw_feedback_edge_lr`].
pub(crate) struct FeedbackRouteLr {
    /// Column where the route leaves the source's bottom border.
    pub(crate) exit_x: usize,
    /// Row of the source's bottom border.
    pub(crate) src_bottom_y: usize,
    /// Gap column right of the source's *column* that carries the drop to the
    /// lane. Boxes are centred in a column sized by its widest node, so a
    /// column edge is the only place guaranteed to be clear of every box.
    pub(crate) exit_lane_x: usize,
    /// Column where the arrowhead enters the destination's bottom border.
    pub(crate) entry_x: usize,
    /// Row of the destination's bottom border.
    pub(crate) dst_bottom_y: usize,
    /// Gap column left of the destination's column that carries the rise from
    /// the lane.
    pub(crate) entry_lane_x: usize,
    /// Row of the horizontal lane in the gutter below the diagram.
    pub(crate) lane_y: usize,
}

fn assign_layers(graph: &Graph, feedback_edges: &HashSet<usize>) -> Vec<Vec<String>> {
    // Build a DAG view by excluding feedback edges.
    let node_ids = graph_node_ids(graph);
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();

    for &id in &node_ids {
        in_degree.entry(id).or_insert(0);
        adj.entry(id).or_default();
    }

    for (idx, edge) in graph.edges.iter().enumerate() {
        if feedback_edges.contains(&idx) {
            continue;
        }
        adj.entry(edge.from.as_str())
            .or_default()
            .push(edge.to.as_str());
        *in_degree.entry(edge.to.as_str()).or_insert(0) += 1;
    }

    // Kahn's topological sort
    let mut queue: VecDeque<&str> = VecDeque::new();
    let mut topo_order: Vec<&str> = Vec::new();
    let mut in_deg = in_degree.clone();
    let mut processed: HashSet<&str> = HashSet::new();

    for &id in &node_ids {
        if in_deg.get(id).copied().unwrap_or(0) == 0 {
            queue.push_back(id);
        }
    }

    while let Some(node) = queue.pop_front() {
        if !processed.insert(node) {
            continue;
        }
        topo_order.push(node);
        if let Some(neighbors) = adj.get(node) {
            for &next in neighbors {
                let deg = in_deg.get_mut(next).unwrap();
                *deg = deg.saturating_sub(1);
                if *deg == 0 {
                    queue.push_back(next);
                }
            }
        }
    }

    // Safety net. With a valid feedback-edge set the graph above is a DAG and
    // Kahn's sort has already visited every node; if that invariant were ever
    // broken, append the stragglers so they are drawn rather than dropped.
    for &id in &node_ids {
        if !processed.contains(id) {
            topo_order.push(id);
            processed.insert(id);
        }
    }

    // Longest-path layer assignment
    let mut node_layer: HashMap<&str, usize> = HashMap::new();
    for &node in &topo_order {
        let mut max_parent_layer: Option<usize> = None;
        for (idx, edge) in graph.edges.iter().enumerate() {
            if feedback_edges.contains(&idx) {
                continue;
            }
            if edge.to == node
                && let Some(&parent_layer) = node_layer.get(edge.from.as_str())
            {
                max_parent_layer =
                    Some(max_parent_layer.map_or(parent_layer, |m: usize| m.max(parent_layer)));
            }
        }
        let layer = max_parent_layer.map_or(0, |m| m + 1);
        node_layer.insert(node, layer);
    }

    let max_layer = node_layer.values().copied().max().unwrap_or(0);
    let mut layers: Vec<Vec<String>> = vec![Vec::new(); max_layer + 1];
    for &node in &topo_order {
        let layer = node_layer[node];
        layers[layer].push(node.to_string());
    }
    layers.retain(|l| !l.is_empty());
    layers
}

fn order_within_layers(layers: &mut [Vec<String>], graph: &Graph, feedback_edges: &HashSet<usize>) {
    // Barycenter heuristic to reduce edge crossings
    for _ in 0..4 {
        // Forward pass
        for i in 1..layers.len() {
            let prev_layer = layers[i - 1].clone();
            let mut positions: Vec<(String, f64)> = Vec::new();

            for node in &layers[i] {
                let mut parent_positions: Vec<f64> = Vec::new();
                for (idx, edge) in graph.edges.iter().enumerate() {
                    if feedback_edges.contains(&idx) {
                        continue;
                    }
                    if edge.to == *node
                        && let Some(pos) = prev_layer.iter().position(|n| n == &edge.from)
                    {
                        parent_positions.push(pos as f64);
                    }
                }
                let avg = if parent_positions.is_empty() {
                    layers[i].iter().position(|n| n == node).unwrap_or(0) as f64
                } else {
                    parent_positions.iter().sum::<f64>() / parent_positions.len() as f64
                };
                positions.push((node.clone(), avg));
            }
            positions.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            layers[i] = positions.into_iter().map(|(n, _)| n).collect();
        }

        // Backward pass
        for i in (0..layers.len().saturating_sub(1)).rev() {
            let next_layer = layers[i + 1].clone();
            let mut positions: Vec<(String, f64)> = Vec::new();

            for node in &layers[i] {
                let mut child_positions: Vec<f64> = Vec::new();
                for (idx, edge) in graph.edges.iter().enumerate() {
                    if feedback_edges.contains(&idx) {
                        continue;
                    }
                    if edge.from == *node
                        && let Some(pos) = next_layer.iter().position(|n| n == &edge.to)
                    {
                        child_positions.push(pos as f64);
                    }
                }
                let avg = if child_positions.is_empty() {
                    layers[i].iter().position(|n| n == node).unwrap_or(0) as f64
                } else {
                    child_positions.iter().sum::<f64>() / child_positions.len() as f64
                };
                positions.push((node.clone(), avg));
            }
            positions.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            layers[i] = positions.into_iter().map(|(n, _)| n).collect();
        }
    }
}

fn graph_node_ids(graph: &Graph) -> Vec<&str> {
    let mut ids = Vec::with_capacity(graph.node_order.len().max(graph.nodes.len()));
    let mut seen = HashSet::new();
    for id in &graph.node_order {
        if seen.insert(id.as_str()) {
            ids.push(id.as_str());
        }
    }
    for id in graph.nodes.keys() {
        if seen.insert(id.as_str()) {
            ids.push(id.as_str());
        }
    }
    ids
}

/// Find the edges that must be set aside to make the graph acyclic.
///
/// Runs a depth-first search from every node in declaration order and reports
/// each edge that points at a node still on the search path (a back edge),
/// plus every self-loop. Removing those edges leaves a DAG, which is what the
/// layer assignment needs. The search keeps its own explicit stack so that a
/// long chain of nodes cannot overflow the native call stack.
fn classify_feedback_edges(graph: &Graph) -> HashSet<usize> {
    #[derive(Clone, Copy)]
    enum VisitState {
        Visiting,
        Visited,
    }

    let node_ids = graph_node_ids(graph);
    let mut adj: HashMap<&str, Vec<(usize, &str)>> = HashMap::new();
    for &id in &node_ids {
        adj.entry(id).or_default();
    }
    for (idx, edge) in graph.edges.iter().enumerate() {
        adj.entry(edge.from.as_str())
            .or_default()
            .push((idx, edge.to.as_str()));
        adj.entry(edge.to.as_str()).or_default();
    }

    let mut state: HashMap<&str, VisitState> = HashMap::new();
    let mut feedback = HashSet::new();
    // Each frame holds a node and the index of its next unexplored out-edge.
    let mut stack: Vec<(&str, usize)> = Vec::new();

    for &root in &node_ids {
        if state.contains_key(root) {
            continue;
        }
        state.insert(root, VisitState::Visiting);
        stack.push((root, 0));

        while let Some(frame) = stack.last_mut() {
            let (node, next) = *frame;
            let edges = &adj[node];
            if next >= edges.len() {
                state.insert(node, VisitState::Visited);
                stack.pop();
                continue;
            }
            frame.1 += 1;

            let (edge_idx, to) = edges[next];
            if to == node {
                feedback.insert(edge_idx);
                continue;
            }
            match state.get(to) {
                None => {
                    state.insert(to, VisitState::Visiting);
                    stack.push((to, 0));
                }
                Some(VisitState::Visiting) => {
                    feedback.insert(edge_idx);
                }
                Some(VisitState::Visited) => {}
            }
        }
    }

    feedback
}

fn node_box_width(node: &Node) -> usize {
    label_box_width(&node.label, node.shape)
}

pub(crate) fn label_box_width(label: &str, shape: NodeShape) -> usize {
    let label_width = label.chars().count();
    let width = match shape {
        NodeShape::Diamond => label_width + 6,
        _ => label_width + 4,
    };
    width.max(7)
}

/// Fit `label` into `width` columns, marking a cut with an ellipsis.
///
/// Some routes have a fixed amount of room for their label (a left-right
/// lane runs between two node columns), and a label that overran it used to
/// paint over the route's own corner and off the edge of the canvas.
pub(crate) fn fit_label(label: &str, width: usize) -> String {
    if label.chars().count() <= width {
        return label.to_string();
    }
    match width {
        0 => String::new(),
        1 => "\u{2026}".to_string(),
        _ => label
            .chars()
            .take(width - 1)
            .chain(std::iter::once('\u{2026}'))
            .collect(),
    }
}

// ───── Canvas ─────

pub(crate) const CONN_UP: u8 = 1;
pub(crate) const CONN_DOWN: u8 = 2;
pub(crate) const CONN_LEFT: u8 = 4;
pub(crate) const CONN_RIGHT: u8 = 8;

pub(crate) fn junction_char(connects: u8) -> char {
    match connects {
        c if c == CONN_UP | CONN_DOWN => '│',
        c if c == CONN_LEFT | CONN_RIGHT => '─',
        c if c == CONN_DOWN | CONN_RIGHT => '┌',
        c if c == CONN_DOWN | CONN_LEFT => '┐',
        c if c == CONN_UP | CONN_RIGHT => '└',
        c if c == CONN_UP | CONN_LEFT => '┘',
        c if c == CONN_UP | CONN_DOWN | CONN_RIGHT => '├',
        c if c == CONN_UP | CONN_DOWN | CONN_LEFT => '┤',
        c if c == CONN_DOWN | CONN_LEFT | CONN_RIGHT => '┬',
        c if c == CONN_UP | CONN_LEFT | CONN_RIGHT => '┴',
        c if c == CONN_UP | CONN_DOWN | CONN_LEFT | CONN_RIGHT => '┼',
        c if c == CONN_UP => '│',
        c if c == CONN_DOWN => '│',
        c if c == CONN_LEFT => '─',
        c if c == CONN_RIGHT => '─',
        _ => '·',
    }
}

#[derive(Clone)]
pub(crate) struct CanvasCell {
    pub(crate) ch: char,
    pub(crate) fg: Option<Color>,
    pub(crate) bg: Option<Color>,
    pub(crate) is_node: bool,
    pub(crate) connects: u8,
}

impl Default for CanvasCell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: None,
            bg: None,
            is_node: false,
            connects: 0,
        }
    }
}

pub(crate) struct Canvas {
    pub(crate) width: usize,
    pub(crate) height: usize,
    cells: Vec<Vec<CanvasCell>>,
}

impl Canvas {
    pub(crate) fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![vec![CanvasCell::default(); width]; height],
        }
    }

    pub(crate) fn set(&mut self, x: usize, y: usize, ch: char, fg: Option<Color>) {
        if y < self.height && x < self.width {
            self.cells[y][x].ch = ch;
            self.cells[y][x].fg = fg;
        }
    }

    pub(crate) fn set_node(&mut self, x: usize, y: usize, ch: char, fg: Option<Color>) {
        if y < self.height && x < self.width {
            self.cells[y][x].ch = ch;
            self.cells[y][x].fg = fg;
            self.cells[y][x].is_node = true;
        }
    }

    pub(crate) fn add_connection(&mut self, x: usize, y: usize, dir: u8, fg: Option<Color>) {
        if y < self.height && x < self.width {
            let cell = &mut self.cells[y][x];
            if !cell.is_node {
                cell.connects |= dir;
                cell.ch = junction_char(cell.connects);
                if fg.is_some() {
                    cell.fg = fg;
                }
            }
        }
    }

    /// Draw one cell of a feedback route.
    ///
    /// Feedback routes are planned to run only through gap rows and gap
    /// columns, which never contain nodes. `add_connection` would silently
    /// skip a node cell and leave a route that looks like it stops at a box,
    /// so under test a violation of that invariant fails instead.
    fn connect_route(&mut self, x: usize, y: usize, dir: u8, fg: Option<Color>) {
        #[cfg(test)]
        if y < self.height && x < self.width {
            assert!(
                !self.cells[y][x].is_node,
                "feedback route runs through the node cell at ({x}, {y})"
            );
        }
        self.add_connection(x, y, dir, fg);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_node(
        &mut self,
        cx: usize,
        y: usize,
        width: usize,
        label: &str,
        shape: NodeShape,
        border_fg: Option<Color>,
        text_fg: Option<Color>,
    ) {
        let x = cx.saturating_sub(width / 2);

        let (tl, tr, bl, br, h, v) = match shape {
            NodeShape::Rectangle => ('┌', '┐', '└', '┘', '─', '│'),
            NodeShape::Rounded | NodeShape::Circle => ('╭', '╮', '╰', '╯', '─', '│'),
            NodeShape::Diamond => ('◆', '◆', '◆', '◆', '─', '│'),
        };

        // Top border
        self.set_node(x, y, tl, border_fg);
        for i in 1..width - 1 {
            self.set_node(x + i, y, h, border_fg);
        }
        self.set_node(x + width - 1, y, tr, border_fg);

        // Content line
        self.set_node(x, y + 1, v, border_fg);
        for i in 1..width - 1 {
            self.set_node(x + i, y + 1, ' ', text_fg);
        }
        let label_chars: Vec<char> = label.chars().collect();
        let padding = (width - 2).saturating_sub(label_chars.len());
        let left_pad = padding / 2;
        for (i, &ch) in label_chars.iter().enumerate() {
            if x + 1 + left_pad + i < x + width - 1 {
                self.set_node(x + 1 + left_pad + i, y + 1, ch, text_fg);
            }
        }
        self.set_node(x + width - 1, y + 1, v, border_fg);

        // Bottom border
        self.set_node(x, y + 2, bl, border_fg);
        for i in 1..width - 1 {
            self.set_node(x + i, y + 2, h, border_fg);
        }
        self.set_node(x + width - 1, y + 2, br, border_fg);
    }

    fn set_node_bg(&mut self, x: usize, y: usize, ch: char, fg: Option<Color>, bg: Option<Color>) {
        if y < self.height && x < self.width {
            self.cells[y][x].ch = ch;
            self.cells[y][x].fg = fg;
            self.cells[y][x].bg = bg;
            self.cells[y][x].is_node = true;
        }
    }

    /// Draw a multi-row card (table-like node) used by the JSON graph view.
    /// Returns the y-coordinate of each content row for edge routing.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_card(
        &mut self,
        left_x: usize,
        top_y: usize,
        width: usize,
        title: &str,
        rows: &[CardDrawRow],
        border_fg: Option<Color>,
        title_fg: Option<Color>,
        key_fg: Option<Color>,
        highlight_rows: &HashSet<usize>,
        highlight_fg: Option<Color>,
        card_highlight_bg: Option<Color>,
    ) -> Vec<usize> {
        if width < 4 {
            return Vec::new();
        }
        let inner = width - 2; // space between │ and │
        let bg = card_highlight_bg;

        // ── top border with title: ╭─ title ─────╮ ──
        self.set_node_bg(left_x, top_y, '╭', border_fg, bg);
        self.set_node_bg(left_x + 1, top_y, '─', border_fg, bg);
        let title_chars: Vec<char> = title.chars().collect();
        let max_title = inner.saturating_sub(3); // "─" + space on each side
        let show_title = title_chars.len().min(max_title);
        self.set_node_bg(left_x + 2, top_y, ' ', title_fg, bg);
        for (i, &ch) in title_chars[..show_title].iter().enumerate() {
            self.set_node_bg(left_x + 3 + i, top_y, ch, title_fg, bg);
        }
        let fill_start = left_x + 3 + show_title;
        self.set_node_bg(fill_start, top_y, ' ', border_fg, bg);
        for x in (fill_start + 1)..(left_x + width - 1) {
            self.set_node_bg(x, top_y, '─', border_fg, bg);
        }
        self.set_node_bg(left_x + width - 1, top_y, '╮', border_fg, bg);

        // ── content rows ──
        let key_col_width = rows
            .iter()
            .map(|r| r.key.chars().count())
            .max()
            .unwrap_or(0)
            .min(inner.saturating_sub(4));

        let mut row_ys = Vec::with_capacity(rows.len());
        for (ri, row) in rows.iter().enumerate() {
            let y = top_y + 1 + ri;
            row_ys.push(y);

            let is_highlight = highlight_rows.contains(&ri);
            let row_key_fg = if is_highlight { highlight_fg } else { key_fg };
            let row_val_fg = if is_highlight {
                highlight_fg
            } else {
                row.value_color
            };

            // left border
            self.set_node_bg(left_x, y, '│', border_fg, bg);

            // space after border
            self.set_node_bg(left_x + 1, y, ' ', row_key_fg, bg);

            // key text
            let key_chars: Vec<char> = row.key.chars().collect();
            let show_key = key_chars.len().min(key_col_width);
            for (i, &ch) in key_chars[..show_key].iter().enumerate() {
                self.set_node_bg(left_x + 2 + i, y, ch, row_key_fg, bg);
            }
            // pad key column
            for i in show_key..key_col_width {
                self.set_node_bg(left_x + 2 + i, y, ' ', row_key_fg, bg);
            }

            // gap between key and value
            let val_start = left_x + 2 + key_col_width + 1;
            self.set_node_bg(val_start - 1, y, ' ', row_val_fg, bg);

            // value text (fill remaining space)
            let val_space = (left_x + width - 1).saturating_sub(val_start + 1);
            if row.is_connector {
                // draw ──▶ at the right edge of the card
                for x in val_start..(left_x + width - 1) {
                    self.set_node_bg(x, y, ' ', row_val_fg, bg);
                }
                // put the arrow near the right border
                let arrow_start = (left_x + width - 1).saturating_sub(4);
                if arrow_start >= val_start {
                    self.set_node_bg(arrow_start, y, '─', row_val_fg, bg);
                    self.set_node_bg(arrow_start + 1, y, '─', row_val_fg, bg);
                    self.set_node_bg(arrow_start + 2, y, '▶', row_val_fg, bg);
                }
            } else {
                let val_chars: Vec<char> = row.value_text.chars().collect();
                let show_val = val_chars.len().min(val_space);
                for (i, &ch) in val_chars[..show_val].iter().enumerate() {
                    self.set_node_bg(val_start + i, y, ch, row_val_fg, bg);
                }
                // pad remaining
                for i in show_val..val_space {
                    self.set_node_bg(val_start + i, y, ' ', row_val_fg, bg);
                }
            }

            // space before right border
            self.set_node_bg(left_x + width - 2, y, ' ', border_fg, bg);
            // right border
            self.set_node_bg(left_x + width - 1, y, '│', border_fg, bg);
        }

        // ── bottom border: ╰─────────╯ ──
        let bot_y = top_y + 1 + rows.len();
        self.set_node_bg(left_x, bot_y, '╰', border_fg, bg);
        for x in (left_x + 1)..(left_x + width - 1) {
            self.set_node_bg(x, bot_y, '─', border_fg, bg);
        }
        self.set_node_bg(left_x + width - 1, bot_y, '╯', border_fg, bg);

        row_ys
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_edge_td(
        &mut self,
        src_cx: usize,
        src_bottom_y: usize,
        dst_cx: usize,
        dst_top_y: usize,
        label: Option<&str>,
        edge_fg: Option<Color>,
        label_fg: Option<Color>,
        bus_y_override: Option<usize>,
    ) {
        if src_bottom_y + 1 >= dst_top_y {
            return;
        }

        // A bent edge draws its horizontal run on `mid_y` and its label on the
        // row above. The caller overrides both when the gap also carries
        // feedback routes, so that neither lands on a reserved row.
        let mid_y = bus_y_override
            .filter(|&y| y > src_bottom_y && y < dst_top_y)
            .unwrap_or(src_bottom_y + 1 + (dst_top_y - src_bottom_y - 1) / 2);

        if src_cx == dst_cx {
            // Straight down
            for y in (src_bottom_y + 1)..dst_top_y {
                self.add_connection(src_cx, y, CONN_UP | CONN_DOWN, edge_fg);
            }
            // Arrow replaces last segment
            self.set(dst_cx, dst_top_y - 1, '▼', edge_fg);

            // Place label beside the vertical line
            if let Some(text) = label {
                let label_y = src_bottom_y + 1;
                for (i, ch) in text.chars().enumerate() {
                    self.set(src_cx + 2 + i, label_y, ch, label_fg);
                }
            }
        } else {
            // Down from source to mid_y
            for y in (src_bottom_y + 1)..mid_y {
                self.add_connection(src_cx, y, CONN_UP | CONN_DOWN, edge_fg);
            }

            // Junction at source column, mid_y
            let src_turn = if dst_cx > src_cx {
                CONN_UP | CONN_RIGHT
            } else {
                CONN_UP | CONN_LEFT
            };
            self.add_connection(src_cx, mid_y, src_turn, edge_fg);

            // Horizontal segment
            let (min_x, max_x) = if src_cx < dst_cx {
                (src_cx, dst_cx)
            } else {
                (dst_cx, src_cx)
            };
            for x in (min_x + 1)..max_x {
                self.add_connection(x, mid_y, CONN_LEFT | CONN_RIGHT, edge_fg);
            }

            // Junction at destination column, mid_y
            let dst_turn = if dst_cx > src_cx {
                CONN_LEFT | CONN_DOWN
            } else {
                CONN_RIGHT | CONN_DOWN
            };
            self.add_connection(dst_cx, mid_y, dst_turn, edge_fg);

            // Down from mid_y to destination
            for y in (mid_y + 1)..dst_top_y {
                self.add_connection(dst_cx, y, CONN_UP | CONN_DOWN, edge_fg);
            }

            // Arrow
            self.set(dst_cx, dst_top_y - 1, '▼', edge_fg);

            // Place label above horizontal segment
            if let Some(text) = label {
                let label_len = text.chars().count();
                let label_start = min_x + (max_x - min_x).saturating_sub(label_len) / 2;
                let label_y = if mid_y > 0 { mid_y - 1 } else { mid_y };
                for (i, ch) in text.chars().enumerate() {
                    let lx = label_start + i;
                    if lx < self.width {
                        self.set(lx, label_y, ch, label_fg);
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_edge_lr(
        &mut self,
        _src_cx: usize,
        src_right_x: usize,
        src_cy: usize,
        dst_left_x: usize,
        dst_cy: usize,
        label: Option<&str>,
        edge_fg: Option<Color>,
        label_fg: Option<Color>,
        mid_x_override: Option<usize>,
    ) {
        if src_right_x + 1 >= dst_left_x {
            return;
        }

        let mid_x =
            mid_x_override.unwrap_or_else(|| src_right_x + 1 + (dst_left_x - src_right_x - 1) / 2);

        if src_cy == dst_cy {
            // Straight right
            for x in (src_right_x + 1)..dst_left_x {
                self.add_connection(x, src_cy, CONN_LEFT | CONN_RIGHT, edge_fg);
            }
            // Arrow replaces last segment
            self.set(dst_left_x - 1, dst_cy, '▶', edge_fg);

            // Label above the horizontal line
            if let Some(text) = label {
                let label_x = src_right_x + 2;
                let label_y = if src_cy > 0 { src_cy - 1 } else { 0 };
                for (i, ch) in text.chars().enumerate() {
                    self.set(label_x + i, label_y, ch, label_fg);
                }
            }
        } else {
            // Right from source to mid_x
            for x in (src_right_x + 1)..mid_x {
                self.add_connection(x, src_cy, CONN_LEFT | CONN_RIGHT, edge_fg);
            }

            // Junction at mid_x, source row
            let src_turn = if dst_cy > src_cy {
                CONN_LEFT | CONN_DOWN
            } else {
                CONN_LEFT | CONN_UP
            };
            self.add_connection(mid_x, src_cy, src_turn, edge_fg);

            // Vertical segment
            let (min_y, max_y) = if src_cy < dst_cy {
                (src_cy, dst_cy)
            } else {
                (dst_cy, src_cy)
            };
            for y in (min_y + 1)..max_y {
                self.add_connection(mid_x, y, CONN_UP | CONN_DOWN, edge_fg);
            }

            // Junction at mid_x, destination row
            let dst_turn = if dst_cy > src_cy {
                CONN_UP | CONN_RIGHT
            } else {
                CONN_DOWN | CONN_RIGHT
            };
            self.add_connection(mid_x, dst_cy, dst_turn, edge_fg);

            // Right from mid_x to destination
            for x in (mid_x + 1)..dst_left_x {
                self.add_connection(x, dst_cy, CONN_LEFT | CONN_RIGHT, edge_fg);
            }

            // Arrow
            self.set(dst_left_x - 1, dst_cy, '▶', edge_fg);

            // Label near the vertical segment
            if let Some(text) = label {
                let label_y = min_y + (max_y - min_y).saturating_sub(1) / 2;
                for (i, ch) in text.chars().enumerate() {
                    self.set(mid_x + 2 + i, label_y, ch, label_fg);
                }
            }
        }
    }

    /// Draw a feedback (back) edge in a top-down diagram.
    ///
    /// The route leaves the source through its bottom border, runs right along
    /// a gap row into a vertical lane beside the diagram, comes back along a
    /// gap row above the destination and enters it through its top border:
    ///
    /// ```text
    ///          ┌───────┐
    ///          │       │
    ///          ▼       │
    ///     ┌────────┐   │
    ///     │  Dst   │   │
    ///     └────────┘   │
    ///          │       │
    ///          ▼       │
    ///     ┌────────┐   │
    ///     │  Src   │   │
    ///     └────────┘   │
    ///            │     │
    ///            └─────┘
    /// ```
    ///
    /// Gap rows never contain nodes, so the route cannot pass through a
    /// sibling of either endpoint. Forward edges it crosses render as
    /// junctions.
    pub(crate) fn draw_feedback_edge_td(&mut self, route: &FeedbackRouteTd, fg: Option<Color>) {
        let r = route;

        // Leave the source: stem down to the gap row, turn, run right to the lane.
        for y in (r.src_bottom_y + 1)..r.exit_y {
            self.connect_route(r.exit_x, y, CONN_UP | CONN_DOWN, fg);
        }
        self.connect_route(r.exit_x, r.exit_y, CONN_UP | CONN_RIGHT, fg);
        for x in (r.exit_x + 1)..r.lane_x {
            self.connect_route(x, r.exit_y, CONN_LEFT | CONN_RIGHT, fg);
        }
        self.connect_route(r.lane_x, r.exit_y, CONN_LEFT | CONN_UP, fg);

        // Up the lane.
        for y in (r.entry_y + 1)..r.exit_y {
            self.connect_route(r.lane_x, y, CONN_UP | CONN_DOWN, fg);
        }

        // Back across the gap row above the destination, then down into it.
        self.connect_route(r.lane_x, r.entry_y, CONN_DOWN | CONN_LEFT, fg);
        for x in (r.entry_x + 1)..r.lane_x {
            self.connect_route(x, r.entry_y, CONN_LEFT | CONN_RIGHT, fg);
        }
        self.connect_route(r.entry_x, r.entry_y, CONN_RIGHT | CONN_DOWN, fg);
        let arrow_y = r.dst_top_y.saturating_sub(1);
        for y in (r.entry_y + 1)..arrow_y {
            self.connect_route(r.entry_x, y, CONN_UP | CONN_DOWN, fg);
        }
        self.set(r.entry_x, arrow_y, '▼', fg);
    }

    /// Draw a feedback (back) edge in a left-right diagram.
    ///
    /// The route leaves the source through its bottom border, drops down the
    /// gap column right of it into a horizontal lane below the diagram, runs
    /// left, rises up the gap column left of the destination and enters it
    /// through its bottom border:
    ///
    /// ```text
    ///     ┌───────┐      ┌───────┐
    ///     │  Dst  │─────▶│  Src  │
    ///     └───────┘      └───────┘
    ///       ▲                  └─┐
    ///     ┌─┘                    │
    ///     └──────────────────────┘
    /// ```
    ///
    /// The drop and the rise use the gap columns beside the node's *column*,
    /// not beside its box. Boxes are centred in a column as wide as its widest
    /// node, so only a column edge is guaranteed clear of every box; a margin
    /// measured from a narrow box can sit inside a wider neighbour.
    pub(crate) fn draw_feedback_edge_lr(&mut self, route: &FeedbackRouteLr, fg: Option<Color>) {
        let r = route;
        let exit_y = r.src_bottom_y + 1;
        let entry_y = r.dst_bottom_y + 2;

        // Leave the source: turn under its bottom border, run right, drop.
        self.connect_route(r.exit_x, exit_y, CONN_UP | CONN_RIGHT, fg);
        for x in (r.exit_x + 1)..r.exit_lane_x {
            self.connect_route(x, exit_y, CONN_LEFT | CONN_RIGHT, fg);
        }
        self.connect_route(r.exit_lane_x, exit_y, CONN_LEFT | CONN_DOWN, fg);
        for y in (exit_y + 1)..r.lane_y {
            self.connect_route(r.exit_lane_x, y, CONN_UP | CONN_DOWN, fg);
        }

        // Along the lane.
        self.connect_route(r.exit_lane_x, r.lane_y, CONN_UP | CONN_LEFT, fg);
        for x in (r.entry_lane_x + 1)..r.exit_lane_x {
            self.connect_route(x, r.lane_y, CONN_LEFT | CONN_RIGHT, fg);
        }
        self.connect_route(r.entry_lane_x, r.lane_y, CONN_UP | CONN_RIGHT, fg);

        // Rise beside the destination, run right under it, then up into it.
        for y in (entry_y + 1)..r.lane_y {
            self.connect_route(r.entry_lane_x, y, CONN_UP | CONN_DOWN, fg);
        }
        self.connect_route(r.entry_lane_x, entry_y, CONN_DOWN | CONN_RIGHT, fg);
        for x in (r.entry_lane_x + 1)..r.entry_x {
            self.connect_route(x, entry_y, CONN_LEFT | CONN_RIGHT, fg);
        }
        self.connect_route(r.entry_x, entry_y, CONN_LEFT | CONN_UP, fg);
        self.set(r.entry_x, r.dst_bottom_y + 1, '▲', fg);
    }

    pub(crate) fn to_span_rows(&self, theme: &Theme) -> Vec<Vec<StyledSpan>> {
        let default_bg = Some(theme.code_bg);
        self.cells
            .iter()
            .map(|row| {
                let mut spans = Vec::new();
                let mut i = 0;
                while i < row.len() {
                    let fg = row[i].fg.unwrap_or(theme.fg);
                    let cell_bg = row[i].bg.or(default_bg);
                    let mut text = String::new();
                    let mut j = i;
                    while j < row.len()
                        && row[j].fg.unwrap_or(theme.fg) == fg
                        && row[j].bg.or(default_bg) == cell_bg
                    {
                        text.push(row[j].ch);
                        j += 1;
                    }
                    spans.push(StyledSpan {
                        text,
                        style: Style {
                            fg: Some(fg),
                            bg: cell_bg,
                            ..Default::default()
                        },
                    });
                    i = j;
                }
                spans
            })
            .collect()
    }
}

// ───── Top-Down rendering ─────

fn render_td(graph: &Graph, theme: &Theme) -> Option<(Vec<Vec<StyledSpan>>, usize)> {
    let node_height: usize = 3;
    let edge_gap: usize = 4;
    let h_gap: usize = 4;

    let layout = layout(graph);
    let layers = &layout.layers;
    if layers.is_empty() {
        return None;
    }
    let feedback = plan_feedback(graph, &layout);
    let last_layer = layers.len() - 1;

    // Calculate node widths
    let mut widths: HashMap<String, usize> = HashMap::new();
    for (id, node) in &graph.nodes {
        widths.insert(id.clone(), node_box_width(node));
    }

    // Find widest layer to determine canvas width
    let mut max_layer_width: usize = 0;
    for layer in layers {
        let w: usize = layer
            .iter()
            .map(|id| widths.get(id).copied().unwrap_or(7))
            .sum::<usize>()
            + layer.len().saturating_sub(1) * h_gap;
        max_layer_width = max_layer_width.max(w);
    }
    let core_width = max_layer_width + 6; // margin on each side

    // ── Row budget ──
    //
    // The gap under layer `k` holds, from the top:
    //
    //   1 row                a straight forward edge's label,
    //   `exits[k]` rows      one per feedback source in layer k,
    //   2 rows               a bent forward edge's label, then its bus,
    //   `entries[k+1]` rows  one per feedback target in layer k+1,
    //   1 row                the forward arrowhead.
    //
    // So every feedback endpoint in the gap owns a row that no other route and
    // no label writes to: two routes can neither merge into one ambiguous line
    // nor be painted over. With no feedback edges this is the original
    // four-row gap, and acyclic diagrams render exactly as they did before.
    let gap_rows: Vec<usize> = (0..last_layer)
        .map(|k| edge_gap + feedback.exits[k] + feedback.entries[k + 1])
        .collect();
    // A route into the first layer or out of the last one needs rows outside
    // the diagram: one per rank, plus one for the arrowhead or the stem.
    let margin = |count: usize| if count == 0 { 0 } else { count + 1 };
    let top_margin = margin(feedback.entries[0]);
    let bottom_margin = margin(feedback.exits[last_layer]);

    // `gap_rows` has one entry per gap, so one fewer than there are layers.
    let mut layer_top: Vec<usize> = Vec::with_capacity(layers.len());
    let mut next_y = top_margin;
    for gap in &gap_rows {
        layer_top.push(next_y);
        next_y += node_height + gap;
    }
    layer_top.push(next_y);
    let canvas_height = next_y + node_height + bottom_margin;

    // ── Gutter ──
    //
    // Lanes sit four columns apart, and a labelled lane additionally reserves
    // the columns its own label occupies, so one long label no longer widens
    // every other lane.
    let label_width = |plan: &FeedbackPlan| {
        graph.edges[plan.edge]
            .label
            .as_ref()
            .map_or(0, |label| label.chars().count())
    };
    let mut lane_xs: Vec<usize> = Vec::with_capacity(feedback.plans.len());
    let mut gutter_x = core_width + 1;
    for plan in &feedback.plans {
        lane_xs.push(gutter_x);
        gutter_x += 4 + label_width(plan);
    }
    let canvas_width = if feedback.plans.is_empty() {
        core_width
    } else {
        gutter_x
    };

    let mut canvas = Canvas::new(canvas_width, canvas_height);

    // Calculate node positions and draw nodes
    let mut positions: HashMap<String, NodeLayout> = HashMap::new();
    let border_fg = Some(theme.code_border);
    let text_fg = Some(theme.fg);

    // First pass: calculate centers for the widest layer
    // Then align single-node layers to the canvas center
    let canvas_center = core_width / 2;

    for (layer_idx, layer) in layers.iter().enumerate() {
        let y = layer_top[layer_idx];

        // Compute node centers relative to layer, then offset to center in canvas
        let node_widths_in_layer: Vec<usize> = layer
            .iter()
            .map(|id| widths.get(id).copied().unwrap_or(7))
            .collect();
        let layer_width: usize =
            node_widths_in_layer.iter().sum::<usize>() + layer.len().saturating_sub(1) * h_gap;

        // Compute center of each node within the layer
        let mut centers_in_layer: Vec<usize> = Vec::new();
        let mut cumulative = 0;
        for &w in &node_widths_in_layer {
            centers_in_layer.push(cumulative + w / 2);
            cumulative += w + h_gap;
        }

        // Center of the layer
        let layer_center = if layer_width > 0 { layer_width / 2 } else { 0 };

        for (i, id) in layer.iter().enumerate() {
            let w = node_widths_in_layer[i];
            // Shift node center so that the layer center aligns with canvas center
            let cx = (canvas_center as isize + centers_in_layer[i] as isize - layer_center as isize)
                .max(w as isize / 2) as usize;

            if let Some(node) = graph.nodes.get(id) {
                canvas.draw_node(cx, y, w, &node.label, node.shape, border_fg, text_fg);
            }

            positions.insert(
                id.clone(),
                NodeLayout {
                    center_x: cx,
                    top_y: y,
                    width: w,
                },
            );
        }
    }

    let edge_fg = Some(theme.code_border);
    let label_fg = Some(theme.h3); // Use a distinct color for edge labels

    // Feedback edges go first so that forward-edge arrowheads, which overwrite
    // cells, end up on top of any route they cross.
    let mut labels: Vec<(usize, usize, String)> = Vec::new();
    for plan in &feedback.plans {
        let edge = &graph.edges[plan.edge];
        let (Some(src), Some(dst)) = (positions.get(&edge.from), positions.get(&edge.to)) else {
            continue;
        };
        let route = FeedbackRouteTd {
            // Leave under the left border. A straight forward edge writes its
            // label two columns right of the box centre on the first gap row,
            // which is the row this stem drops through.
            exit_x: src.left_x() + 1,
            src_bottom_y: src.bottom_y(),
            exit_y: src.bottom_y() + 2 + plan.src_rank,
            entry_x: dst.right_x().saturating_sub(1),
            dst_top_y: dst.top_y,
            entry_y: dst.top_y.saturating_sub(2 + plan.dst_rank),
            lane_x: lane_xs[plan.lane],
        };
        canvas.draw_feedback_edge_td(&route, edge_fg);

        if let Some(text) = edge.label.as_deref() {
            // Beside this edge's lane, level with the middle of its vertical run.
            let y = (route.entry_y + route.exit_y) / 2;
            labels.push((route.lane_x + 2, y, text.to_string()));
        }
    }
    for (x, y, text) in &labels {
        for (i, ch) in text.chars().enumerate() {
            canvas.set(x + i, *y, ch, label_fg);
        }
    }

    // Forward edges
    for (idx, edge) in graph.edges.iter().enumerate() {
        if layout.feedback_set.contains(&idx) {
            continue;
        }
        if let (Some(src), Some(dst)) = (positions.get(&edge.from), positions.get(&edge.to)) {
            // Between adjacent layers the bus sits below the gap's exit rows
            // and above its entry rows, so neither it nor the label on the row
            // above it can land on a feedback route.
            let bus_y = match (
                layout.node_pos.get(&edge.from),
                layout.node_pos.get(&edge.to),
            ) {
                (Some(&(from, _)), Some(&(to, _))) if to == from + 1 => {
                    Some(src.bottom_y() + 3 + feedback.exits[from])
                }
                _ => None,
            };
            canvas.draw_edge_td(
                src.center_x,
                src.bottom_y(),
                dst.center_x,
                dst.top_y,
                edge.label.as_deref(),
                edge_fg,
                label_fg,
                bus_y,
            );
        }
    }

    let rows = canvas.to_span_rows(theme);
    Some((rows, canvas_width))
}

// ───── Left-Right rendering ─────

fn render_lr(graph: &Graph, theme: &Theme) -> Option<(Vec<Vec<StyledSpan>>, usize)> {
    let node_height: usize = 3;
    let node_h_gap: usize = 6; // horizontal gap between columns for edge routing
    let v_gap: usize = 2; // vertical gap between nodes in same column
    let lane_gap: usize = 2; // rows between gutter lanes

    let layout = layout(graph);
    let layers = &layout.layers;
    if layers.is_empty() {
        return None;
    }
    let feedback = plan_feedback(graph, &layout);
    let last_layer = layers.len() - 1;

    // Calculate node widths
    let mut widths: HashMap<String, usize> = HashMap::new();
    for (id, node) in &graph.nodes {
        widths.insert(id.clone(), node_box_width(node));
    }

    // Column widths (max node width per layer)
    let col_widths: Vec<usize> = layers
        .iter()
        .map(|layer| {
            layer
                .iter()
                .map(|id| widths.get(id).copied().unwrap_or(7))
                .max()
                .unwrap_or(7)
        })
        .collect();

    let max_nodes_in_layer = layers.iter().map(|l| l.len()).max().unwrap_or(1);

    // ── Column budget ──
    //
    // The gap right of column `k` carries the drop column of every feedback
    // source in column k and the rise column of every feedback target in
    // column k+1, and still has to leave the forward edges' bend column clear
    // between them. With no feedback edges the gap is the original six
    // columns, so acyclic diagrams are unaffected.
    let gap_widths: Vec<usize> = (0..last_layer)
        .map(|k| {
            let exits = feedback.exits[k];
            let entries = feedback.entries[k + 1];
            node_h_gap
                .max(exits + entries + 4)
                .max(2 * exits)
                .max(2 * entries + 2)
        })
        .collect();
    // Routes into the first column or out of the last one use the margins.
    let left_margin = 2.max(feedback.entries[0]);
    let right_margin = 2.max(feedback.exits[last_layer]);

    let canvas_width: usize = left_margin
        + col_widths.iter().sum::<usize>()
        + gap_widths.iter().sum::<usize>()
        + right_margin;
    let core_height = max_nodes_in_layer * (node_height + v_gap) - v_gap + 2;
    // One spare row under the diagram, then a lane every `lane_gap` rows.
    let gutter_height = if feedback.plans.is_empty() {
        0
    } else {
        lane_gap * feedback.plans.len() + 1
    };
    let canvas_height = core_height + gutter_height;

    let mut canvas = Canvas::new(canvas_width, canvas_height);

    let mut positions: HashMap<String, NodeLayout> = HashMap::new();
    let border_fg = Some(theme.code_border);
    let text_fg = Some(theme.fg);

    // (left, right) border columns of each layer's column.
    let mut col_bounds: Vec<(usize, usize)> = Vec::with_capacity(layers.len());
    let mut col_x = left_margin;
    for (layer_idx, layer) in layers.iter().enumerate() {
        let col_w = col_widths[layer_idx];
        col_bounds.push((col_x, col_x + col_w - 1));

        let total_layer_height = layer.len() * node_height + layer.len().saturating_sub(1) * v_gap;
        let start_y = (core_height.saturating_sub(total_layer_height)) / 2;

        for (node_idx, id) in layer.iter().enumerate() {
            let w = widths.get(id).copied().unwrap_or(7);
            let cx = col_x + col_w / 2;
            let y = start_y + node_idx * (node_height + v_gap);

            if let Some(node) = graph.nodes.get(id) {
                canvas.draw_node(cx, y, w, &node.label, node.shape, border_fg, text_fg);
            }

            positions.insert(
                id.clone(),
                NodeLayout {
                    center_x: cx,
                    top_y: y,
                    width: w,
                },
            );
        }

        col_x += col_w + gap_widths.get(layer_idx).copied().unwrap_or(0);
    }

    let edge_fg = Some(theme.code_border);
    let label_fg = Some(theme.h3);

    // Feedback edges go first so that forward-edge arrowheads, which overwrite
    // cells, end up on top of any route they cross.
    let lane_y = |lane: usize| core_height + 1 + lane * lane_gap;
    let mut labels: Vec<(usize, usize, String)> = Vec::new();
    for plan in &feedback.plans {
        let edge = &graph.edges[plan.edge];
        let (Some(src), Some(dst)) = (positions.get(&edge.from), positions.get(&edge.to)) else {
            continue;
        };
        let (Some(&(src_layer, _)), Some(&(dst_layer, _))) = (
            layout.node_pos.get(&edge.from),
            layout.node_pos.get(&edge.to),
        ) else {
            continue;
        };
        // Drop and rise beside the node's column rather than beside its box: a
        // narrow box's own margin can sit inside a wider box stacked with it,
        // and a route through a box is drawn as if it were not there at all.
        // One column per rank keeps stacked endpoints on separate routes.
        let route = FeedbackRouteLr {
            exit_x: src.right_x().saturating_sub(1),
            src_bottom_y: src.bottom_y(),
            exit_lane_x: col_bounds[src_layer].1 + 1 + plan.src_rank,
            entry_x: dst.left_x() + 1,
            dst_bottom_y: dst.bottom_y(),
            entry_lane_x: col_bounds[dst_layer].0.saturating_sub(1 + plan.dst_rank),
            lane_y: lane_y(plan.lane),
        };
        canvas.draw_feedback_edge_lr(&route, edge_fg);

        if let Some(text) = edge.label.as_deref() {
            // Inline on the lane, centered on its horizontal run. The run is
            // all the room there is, so a longer label is cut rather than
            // written over the route's own corner.
            let inner = route.exit_lane_x.saturating_sub(route.entry_lane_x + 1);
            let text = fit_label(text, inner);
            let x = route.entry_lane_x + 1 + inner.saturating_sub(text.chars().count()) / 2;
            labels.push((x, route.lane_y, text));
        }
    }
    for (x, y, text) in &labels {
        for (i, ch) in text.chars().enumerate() {
            canvas.set(x + i, *y, ch, label_fg);
        }
    }

    // Forward edges
    for (idx, edge) in graph.edges.iter().enumerate() {
        if layout.feedback_set.contains(&idx) {
            continue;
        }
        if let (Some(src), Some(dst)) = (positions.get(&edge.from), positions.get(&edge.to)) {
            // Bend in the middle of the gap between the two columns, so that
            // every edge between them turns in the same column no matter how
            // wide the individual nodes are.
            let mid_x = match (
                layout.node_pos.get(&edge.from),
                layout.node_pos.get(&edge.to),
            ) {
                (Some(&(src_layer, _)), Some(&(dst_layer, _))) => {
                    let src_right = col_bounds[src_layer].1;
                    let dst_left = col_bounds[dst_layer].0;
                    (dst_left > src_right + 1)
                        .then(|| src_right + 1 + (dst_left - src_right - 1) / 2)
                }
                _ => None,
            };
            canvas.draw_edge_lr(
                src.center_x,
                src.right_x(),
                src.top_y + 1,
                dst.left_x(),
                dst.top_y + 1,
                edge.label.as_deref(),
                edge_fg,
                label_fg,
                mid_x,
            );
        }
    }

    let rows = canvas.to_span_rows(theme);
    Some((rows, canvas_width))
}

// ───── Public API ─────

/// Try to render mermaid code as a visual diagram.
/// Returns (content_rows, content_width) or None if parsing fails.
pub fn render_mermaid(code: &str, theme: &Theme) -> Option<(Vec<Vec<StyledSpan>>, usize)> {
    let graph = parse_mermaid(code)?;
    match graph.direction {
        Direction::TopDown => render_td(&graph, theme),
        Direction::LeftRight => render_lr(&graph, theme),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    fn render_text(code: &str) -> String {
        let theme = Theme::dark();
        let (rows, _) = render_mermaid(code, &theme).expect("diagram should render");
        rows.into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|span| span.text)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Render on a helper thread so that a layout that never terminates fails
    /// the test instead of hanging the whole test binary.
    fn render_text_with_timeout(code: &'static str) -> String {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(render_text(code));
        });
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(text) => text,
            // The sender is dropped as soon as the render thread unwinds, so a
            // panic there arrives here as a disconnect, not as a timeout.
            Err(mpsc::RecvTimeoutError::Disconnected) => panic!("the render thread panicked"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("rendering did not finish within 10 seconds")
            }
        }
    }

    /// Compare a render against a snapshot written at column 0 inside a raw
    /// string literal (one leading newline, trailing blank rows ignored).
    ///
    /// Every render also checks an invariant of its own: [`Canvas::connect_route`]
    /// fails the test if a feedback route is laid over a node cell, which is
    /// what a route that runs through a box looks like.
    fn assert_render(code: &'static str, expected: &str) {
        let text = render_text_with_timeout(code);
        let expected = expected.strip_prefix('\n').unwrap_or(expected);
        assert_eq!(
            text.trim_end(),
            expected.trim_end(),
            "\n--- rendered ---\n{text}\n--- expected ---\n{expected}"
        );
    }

    /// A linear chain N0 -> N1 -> ... -> N{n-1}, optionally closed into a cycle.
    fn chain_graph(n: usize, close_cycle: bool) -> Graph {
        let ids: Vec<String> = (0..n).map(|i| format!("N{i}")).collect();
        let nodes = ids
            .iter()
            .map(|id| {
                (
                    id.clone(),
                    Node {
                        label: id.clone(),
                        shape: NodeShape::Rectangle,
                    },
                )
            })
            .collect();
        let mut edges: Vec<Edge> = ids
            .windows(2)
            .map(|pair| Edge {
                from: pair[0].clone(),
                to: pair[1].clone(),
                label: None,
            })
            .collect();
        if close_cycle {
            edges.push(Edge {
                from: ids[n - 1].clone(),
                to: ids[0].clone(),
                label: None,
            });
        }
        Graph {
            direction: Direction::TopDown,
            nodes,
            edges,
            node_order: ids,
        }
    }

    /// Render a deterministic spread of graph shapes and let
    /// [`Canvas::connect_route`] check every feedback route against every box.
    ///
    /// The shapes vary in node count, node width, edge count, direction and
    /// labelling, because a route only collides with a box that a *differently
    /// sized* neighbour widened the column for.
    #[test]
    fn feedback_routes_never_run_through_a_node_box() {
        // xorshift with a fixed seed, so a failure is always reproducible.
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for case in 0..2000u32 {
            let n = 2 + (next() % 14) as usize;
            let lr = next() % 2 == 0;
            let mut code = String::from(if lr { "graph LR\n" } else { "graph TD\n" });
            let names: Vec<String> = (0..n)
                .map(|i| format!("N{i}[{}]", "x".repeat(1 + (i % 4) * 5)))
                .collect();
            for _ in 0..1 + (next() % 30) {
                let a = (next() % n as u64) as usize;
                let b = (next() % n as u64) as usize;
                if next() % 3 == 0 {
                    let label = "L".repeat(1 + (next() % 12) as usize);
                    code.push_str(&format!("  {} -->|{label}| {}\n", names[a], names[b]));
                } else {
                    code.push_str(&format!("  {} --> {}\n", names[a], names[b]));
                }
            }

            let theme = Theme::dark();
            let leaked: &'static str = Box::leak(code.clone().into_boxed_str());
            let (tx, rx) = mpsc::channel();
            let handle = thread::spawn(move || {
                let _ = tx.send(render_mermaid(leaked, &theme).is_some());
            });
            if let Err(err) = rx.recv_timeout(Duration::from_secs(20)) {
                let _ = handle.join();
                panic!("case {case} failed ({err:?}) for:\n{code}");
            }
            handle.join().unwrap();
        }
    }

    // ── Cycle classification ──

    #[test]
    fn closing_edge_of_a_declared_cycle_is_the_feedback_edge() {
        let graph = parse_mermaid("graph TD\n    A --> B\n    B --> C\n    C --> A\n").unwrap();
        let feedback = classify_feedback_edges(&graph);
        assert_eq!(feedback.into_iter().collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn deep_chain_does_not_overflow_the_stack() {
        let acyclic = chain_graph(200_000, false);
        assert!(classify_feedback_edges(&acyclic).is_empty());

        let cyclic = chain_graph(200_000, true);
        let feedback = classify_feedback_edges(&cyclic);
        assert_eq!(feedback.len(), 1);
        assert!(feedback.contains(&(cyclic.edges.len() - 1)));
    }

    #[test]
    fn longer_feedback_edges_take_outer_lanes() {
        let graph = parse_mermaid(
            "graph TD\n    Start --> Check\n    Check --> Work\n    Work --> Done\n    Done --> Start\n    Work --> Check\n",
        )
        .unwrap();
        let layout = layout(&graph);
        // Work -> Check spans one layer, Done -> Start spans three.
        assert_eq!(layout.feedback, vec![4, 3]);
    }

    #[test]
    fn feedback_endpoints_are_counted_per_layer() {
        let graph = parse_mermaid("graph TD\n    S --> A\n    S --> B\n    A --> S\n    B --> S\n")
            .unwrap();
        let layout = layout(&graph);
        let feedback = plan_feedback(&graph, &layout);
        // Both back edges leave the second layer and enter the first.
        assert_eq!(feedback.exits, vec![0, 2]);
        assert_eq!(feedback.entries, vec![1, 0]);
        // Two sources in one layer get distinct ranks.
        let mut ranks: Vec<usize> = feedback.plans.iter().map(|p| p.src_rank).collect();
        ranks.sort_unstable();
        assert_eq!(ranks, vec![0, 1]);
    }

    // ── Acyclic rendering must not change ──

    #[test]
    fn acyclic_top_down_diagram() {
        assert_render(
            "graph TD\n    A[Start] --> B{Decision}\n    B -->|Yes| C[Action 1]\n    B -->|No| D[Action 2]\n    C --> E[End]\n    D --> E\n",
            r#"
             ┌───────┐
             │ Start │
             └───────┘
                 │
                 │
                 │
                 ▼
          ◆────────────◆
          │  Decision  │
          ◆────────────◆
                 │
           Yes   │  No
         ┌───────┴───────┐
         ▼               ▼
   ┌──────────┐    ┌──────────┐
   │ Action 1 │    │ Action 2 │
   └──────────┘    └──────────┘
         │               │
         │               │
         └───────┬───────┘
                 ▼
              ┌─────┐
              │ End │
              └─────┘
"#,
        );
    }

    #[test]
    fn acyclic_left_right_diagram() {
        assert_render(
            "graph LR\n    A[Start] --> B{Decision}\n    B -->|Yes| C[Action 1]\n    B -->|No| D[Do]\n    C --> E[End]\n    D --> E\n",
            r#"

                                     ┌──────────┐
                                  ┌─YesAction 1 │───┐
  ┌───────┐      ◆────────────◆   │  └──────────┘   │  ┌─────┐
  │ Start │─────▶│  Decision  │───┤                 ├─▶│ End │
  └───────┘      ◆────────────◆   │ No              │  └─────┘
                                  │     ┌─────┐     │
                                  └────▶│ Do  │─────┘
                                        └─────┘

"#,
        );
    }

    // ── Cycles ──

    #[test]
    fn top_down_cycle_wraps_around_the_right() {
        assert_render(
            "graph TB\n    Loop --> Execute\n    Execute --> Repeat\n    Repeat --> Loop\n",
            r#"
          ┌───────┐
          ▼       │
    ┌──────┐      │
    │ Loop │      │
    └──────┘      │
        │         │
        │         │
        │         │
        ▼         │
   ┌─────────┐    │
   │ Execute │    │
   └─────────┘    │
        │         │
        │         │
        │         │
        ▼         │
   ┌────────┐     │
   │ Repeat │     │
   └────────┘     │
    │             │
    └─────────────┘
"#,
        );
    }

    #[test]
    fn left_right_cycle_wraps_underneath() {
        assert_render(
            "graph LR\n    A[Start] --> B[End]\n    B --> A\n",
            r#"

  ┌───────┐      ┌─────┐
  │ Start │─────▶│ End │
  └───────┘      └─────┘
   ▲                  └─┐
 ┌─┘                    │
 └──────────────────────┘

"#,
        );
    }

    #[test]
    fn self_loop_top_down_is_a_closed_loop() {
        assert_render(
            "graph TD\n    A[Self] --> A\n",
            r#"
         ┌─────┐
         ▼     │
   ┌──────┐    │
   │ Self │    │
   └──────┘    │
    │          │
    └──────────┘
"#,
        );
    }

    #[test]
    fn self_loop_left_right_is_a_closed_loop() {
        assert_render(
            "graph LR\n    A[Self] --> A\n",
            r#"

  ┌──────┐
  │ Self │
  └──────┘
   ▲    └─┐
 ┌─┘      │
 └────────┘

"#,
        );
    }

    // ── Routes must not pass through other nodes ──

    #[test]
    fn feedback_edge_avoids_sibling_nodes_top_down() {
        // The back edge into B leaves D, wraps around the gutter and comes back
        // through a gap row. It never touches the row C is drawn on.
        assert_render(
            "graph TD\n    A --> B\n    A --> C\n    B --> D\n    C --> D\n    D --> B\n",
            r#"
         ┌─────┐
         │  A  │
         └─────┘
            │
            │
      ┌─────┴────┐
      │ ┌────────┼───────┐
      ▼ ▼        ▼       │
   ┌─────┐    ┌─────┐    │
   │  B  │    │  C  │    │
   └─────┘    └─────┘    │
      │          │       │
      │          │       │
      └─────┬────┘       │
            ▼            │
         ┌─────┐         │
         │  D  │         │
         └─────┘         │
          │              │
          └──────────────┘
"#,
        );
    }

    #[test]
    fn feedback_edge_avoids_stacked_nodes_left_right() {
        // C sits directly below B; the route into B rises beside their column.
        assert_render(
            "graph LR\n    A --> B\n    A --> C\n    B --> D\n    C --> D\n    D --> B\n",
            r#"

               ┌─────┐
            ┌─▶│  B  │───┐
  ┌─────┐   │  └─────┘   │  ┌─────┐
  │  A  │───┤   ▲        ├─▶│  D  │
  └─────┘   │ ┌─┘        │  └─────┘
            │ │┌─────┐   │       └─┐
            └─▶│  C  │───┘         │
              │└─────┘             │
              │                    │
              │                    │
              └────────────────────┘

"#,
        );
    }

    #[test]
    fn feedback_route_clears_a_wider_stacked_node_left_right() {
        // B is narrower than the node stacked under it. Its own right margin is
        // inside that wider box, so the route has to use the column edge; drawing
        // it over the box would leave a line that stops dead at the border.
        assert_render(
            "graph LR\n    A --> B\n    A --> C[VeryLongName]\n    B --> D\n    C --> D\n    D --> B\n",
            r#"

                    ┌─────┐
            ┌──────▶│  B  │───────┐
  ┌─────┐   │       └─────┘       │  ┌─────┐
  │  A  │───┤        ▲            ├─▶│  D  │
  └─────┘   │ ┌──────┘            │  └─────┘
            │ │┌──────────────┐   │       └─┐
            └─▶│ VeryLongName │───┘         │
              │└──────────────┘             │
              │                             │
              │                             │
              └─────────────────────────────┘

"#,
        );
    }

    // ── Endpoints that share a layer, and gaps that carry both ──

    #[test]
    fn feedback_route_clears_a_wider_stacked_source_left_right() {
        // The mirror of the case above: B leaves through a column edge because
        // its own right margin is inside the wider box stacked under it. The
        // crossing with C -> D is a junction, not a break.
        assert_render(
            "graph LR\n    A --> B\n    A --> C[VeryLongName]\n    B --> D\n    C --> D\n    B --> A\n",
            r#"

                    ┌─────┐
            ┌──────▶│  B  │───────┐
  ┌─────┐   │       └─────┘       │  ┌─────┐
  │  A  │───┤            └─────┐  ├─▶│  D  │
  └─────┘   │                  │  │  └─────┘
   ▲        │  ┌──────────────┐│  │
 ┌─┘        └─▶│ VeryLongName │┼──┘
 │             └──────────────┘│
 │                             │
 │                             │
 └─────────────────────────────┘

"#,
        );
    }

    #[test]
    fn two_feedback_sources_in_one_layer_leave_on_different_rows() {
        assert_render(
            "graph TD\n    S --> A\n    S --> B\n    A --> S\n    B --> S\n",
            r#"
              ┌──────────┬───┐
              ▼          │   │
         ┌─────┐         │   │
         │  S  │         │   │
         └─────┘         │   │
            │            │   │
            │            │   │
      ┌─────┴────┐       │   │
      ▼          ▼       │   │
   ┌─────┐    ┌─────┐    │   │
   │  A  │    │  B  │    │   │
   └─────┘    └─────┘    │   │
    │          │         │   │
    │          └─────────┼───┘
    └────────────────────┘
"#,
        );
    }

    #[test]
    fn three_feedback_sources_in_one_layer_leave_on_different_rows() {
        // A third source needs a third row: one row per rank, not one row for the
        // nearest source and one shared by all the rest.
        assert_render(
            "graph TD\n    S --> A\n    S --> B\n    S --> C\n    A --> S\n    B --> S\n    C --> S\n",
            r#"
                   ┌────────────────┬───┬───┐
                   ▼                │   │   │
              ┌─────┐               │   │   │
              │  S  │               │   │   │
              └─────┘               │   │   │
                 │                  │   │   │
                 │                  │   │   │
      ┌──────────┼──────────┐       │   │   │
      ▼          ▼          ▼       │   │   │
   ┌─────┐    ┌─────┐    ┌─────┐    │   │   │
   │  A  │    │  B  │    │  C  │    │   │   │
   └─────┘    └─────┘    └─────┘    │   │   │
    │          │          │         │   │   │
    │          │          └─────────┼───┼───┘
    │          └────────────────────┼───┘
    └───────────────────────────────┘
"#,
        );
    }

    #[test]
    fn two_feedback_sources_in_one_column_drop_in_different_columns() {
        assert_render(
            "graph LR\n    S --> A\n    S --> B\n    A --> S\n    B --> S\n",
            r#"

               ┌─────┐
            ┌─▶│  A  │
  ┌─────┐   │  └─────┘
  │  S  │───┤       └──┐
  └─────┘   │          │
   ▲        │  ┌─────┐ │
 ┌─┘        └─▶│  B  │ │
 │             └─────┘ │
 │                  └─┐│
 │                    ││
 ├────────────────────┼┘
 │                    │
 └────────────────────┘

"#,
        );
    }

    #[test]
    fn feedback_exit_and_entry_in_one_gap_use_different_rows() {
        // B is a feedback source and C, one layer below it, is a feedback target,
        // so one gap carries both. Sharing a row would fuse the two routes into a
        // single line that reads as neither.
        assert_render(
            "graph TD\n    A --> B\n    B --> C\n    C --> D\n    D --> C\n    B --> A\n",
            r#"
        ┌─────────┐
        ▼         │
   ┌─────┐        │
   │  A  │        │
   └─────┘        │
      │           │
      │           │
      │           │
      ▼           │
   ┌─────┐        │
   │  B  │        │
   └─────┘        │
    │ │           │
    └─┼───────────┘
      │
      │
      │ ┌─────┐
      ▼ ▼     │
   ┌─────┐    │
   │  C  │    │
   └─────┘    │
      │       │
      │       │
      │       │
      ▼       │
   ┌─────┐    │
   │  D  │    │
   └─────┘    │
    │         │
    └─────────┘
"#,
        );
    }

    // ── Labels ──

    #[test]
    fn feedback_labels_sit_beside_their_own_lane_top_down() {
        assert_render(
            "graph TD\n    Start --> Check\n    Check --> Work\n    Work --> Done\n    Done -->|retry| Start\n    Work -->|again| Check\n",
            r#"
          ┌──────────────┐
          ▼              │
   ┌───────┐             │
   │ Start │             │
   └───────┘             │
       │                 │
       │                 │
       │                 │
       │  ┌─────┐        │
       ▼  ▼     │        │
   ┌───────┐    │        │
   │ Check │    │        │
   └───────┘    │        │
       │        │        │
       │        │ again  │ retry
       │        │        │
       ▼        │        │
   ┌──────┐     │        │
   │ Work │     │        │
   └──────┘     │        │
    │  │        │        │
    └──┼────────┘        │
       │                 │
       │                 │
       ▼                 │
   ┌──────┐              │
   │ Done │              │
   └──────┘              │
    │                    │
    └────────────────────┘
"#,
        );
    }

    #[test]
    fn feedback_labels_sit_inline_on_their_lane_left_right() {
        assert_render(
            "graph LR\n    Start --> Check\n    Check --> Work\n    Work --> Done\n    Done -->|retry| Start\n    Work -->|again| Check\n",
            r#"

  ┌───────┐      ┌───────┐      ┌──────┐      ┌──────┐
  │ Start │─────▶│ Check │─────▶│ Work │─────▶│ Done │
  └───────┘      └───────┘      └──────┘      └──────┘
   ▲              ▲                   └─┐           └─┐
 ┌─┘            ┌─┘                     │             │
 │              └─────────again─────────┘             │
 │                                                    │
 └───────────────────────retry────────────────────────┘

"#,
        );
    }

    #[test]
    fn a_label_longer_than_its_lane_is_truncated() {
        // The lane runs between two node columns and that is all the room there
        // is, so the label is cut rather than written over the corner and off
        // the edge of the canvas.
        assert_render(
            "graph LR\n    A -->|this is a long label| A\n",
            r#"

  ┌─────┐
  │  A  │
  └─────┘
   ▲   └─┐
 ┌─┘     │
 └this i…┘

"#,
        );
    }

    #[test]
    fn straight_forward_edge_label_clears_the_feedback_stem() {
        // The label on B -> C sits on the first gap row, which is the row the
        // feedback stem drops through. The stem leaves under the left border.
        assert_render(
            "graph TD\n    A --> B\n    B -->|ok| C\n    B --> A\n",
            r#"
        ┌─────┐
        ▼     │
   ┌─────┐    │
   │  A  │    │
   └─────┘    │
      │       │
      │       │
      │       │
      ▼       │
   ┌─────┐    │
   │  B  │    │
   └─────┘    │
    │ │ ok    │
    └─┼───────┘
      │
      │
      ▼
   ┌─────┐
   │  C  │
   └─────┘
"#,
        );
    }

    #[test]
    fn bent_forward_edge_label_clears_the_feedback_route() {
        // The yes/no labels sit on the row above the bus, which the row budget
        // keeps below every exit row in the gap.
        assert_render(
            "graph TD\n    A --> B\n    B -->|yes| C\n    B -->|no| D\n    C --> E\n    D --> E\n    B --> A\n",
            r#"
              ┌──────────┐
              ▼          │
         ┌─────┐         │
         │  A  │         │
         └─────┘         │
            │            │
            │            │
            │            │
            ▼            │
         ┌─────┐         │
         │  B  │         │
         └─────┘         │
          │ │            │
          └─┼────────────┘
       yes  │no
      ┌─────┴────┐
      ▼          ▼
   ┌─────┐    ┌─────┐
   │  C  │    │  D  │
   └─────┘    └─────┘
      │          │
      │          │
      └─────┬────┘
            ▼
         ┌─────┐
         │  E  │
         └─────┘
"#,
        );
    }

    #[test]
    fn one_long_label_does_not_widen_every_lane() {
        let code = "graph TD\n    Start --> Check\n    Check --> Work\n    Work --> Done\n    Done -->|x| Start\n    Work -->|a very long label| Check\n";
        let text = render_text(code);
        let width = text.lines().map(|l| l.chars().count()).max().unwrap_or(0);
        // Each lane reserves its own label's width. Charging every lane for the
        // longest label would add another 16 columns here.
        assert!(width < 45, "gutter is {width} columns wide:\n{text}");
        assert_render(
            code,
            r#"
          ┌──────────────────────────┐
          ▼                          │
   ┌───────┐                         │
   │ Start │                         │
   └───────┘                         │
       │                             │
       │                             │
       │                             │
       │  ┌─────┐                    │
       ▼  ▼     │                    │
   ┌───────┐    │                    │
   │ Check │    │                    │
   └───────┘    │                    │
       │        │                    │
       │        │ a very long label  │ x
       │        │                    │
       ▼        │                    │
   ┌──────┐     │                    │
   │ Work │     │                    │
   └──────┘     │                    │
    │  │        │                    │
    └──┼────────┘                    │
       │                             │
       │                             │
       ▼                             │
   ┌──────┐                          │
   │ Done │                          │
   └──────┘                          │
    │                                │
    └────────────────────────────────┘
"#,
        );
    }
}
