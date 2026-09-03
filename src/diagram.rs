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
    let mut feedback: Vec<usize> = feedback_set.into_iter().collect();
    feedback.sort_by_key(|&idx| (span(idx), idx));

    Layout {
        layers,
        node_pos,
        feedback,
    }
}

/// Routing decisions for one feedback edge.
struct FeedbackPlan {
    /// Index into `graph.edges`.
    edge: usize,
    /// Gutter lane, 0 = innermost.
    lane: usize,
    /// Rank of the source among the feedback sources in its layer, counted
    /// from the gutter side (0 = nearest the gutter). Sources that share a
    /// layer leave on different rows or columns so their routes do not merge.
    src_rank: usize,
    /// Same for the destination among the feedback targets in its layer.
    dst_rank: usize,
}

fn plan_feedback(graph: &Graph, layout: &Layout) -> Vec<FeedbackPlan> {
    let mut sources: HashMap<usize, BTreeSet<usize>> = HashMap::new();
    let mut targets: HashMap<usize, BTreeSet<usize>> = HashMap::new();
    for &idx in &layout.feedback {
        let edge = &graph.edges[idx];
        if let Some(&(layer, pos)) = layout.node_pos.get(&edge.from) {
            sources.entry(layer).or_default().insert(pos);
        }
        if let Some(&(layer, pos)) = layout.node_pos.get(&edge.to) {
            targets.entry(layer).or_default().insert(pos);
        }
    }
    let rank = |set: &HashMap<usize, BTreeSet<usize>>, layer: usize, pos: usize| {
        set.get(&layer)
            .map_or(0, |positions| positions.range(pos + 1..).count())
    };

    layout
        .feedback
        .iter()
        .enumerate()
        .filter_map(|(lane, &idx)| {
            let edge = &graph.edges[idx];
            let &(src_layer, src_pos) = layout.node_pos.get(&edge.from)?;
            let &(dst_layer, dst_pos) = layout.node_pos.get(&edge.to)?;
            Some(FeedbackPlan {
                edge: idx,
                lane,
                src_rank: rank(&sources, src_layer, src_pos),
                dst_rank: rank(&targets, dst_layer, dst_pos),
            })
        })
        .collect()
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
    /// Gap column right of the source that carries the drop to the lane.
    pub(crate) exit_lane_x: usize,
    /// Column where the arrowhead enters the destination's bottom border.
    pub(crate) entry_x: usize,
    /// Row of the destination's bottom border.
    pub(crate) dst_bottom_y: usize,
    /// Gap column left of the destination that carries the rise from the lane.
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
    ) {
        if src_bottom_y + 1 >= dst_top_y {
            return;
        }

        let mid_y = src_bottom_y + 1 + (dst_top_y - src_bottom_y - 1) / 2;

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
            self.add_connection(r.exit_x, y, CONN_UP | CONN_DOWN, fg);
        }
        self.add_connection(r.exit_x, r.exit_y, CONN_UP | CONN_RIGHT, fg);
        for x in (r.exit_x + 1)..r.lane_x {
            self.add_connection(x, r.exit_y, CONN_LEFT | CONN_RIGHT, fg);
        }
        self.add_connection(r.lane_x, r.exit_y, CONN_LEFT | CONN_UP, fg);

        // Up the lane.
        for y in (r.entry_y + 1)..r.exit_y {
            self.add_connection(r.lane_x, y, CONN_UP | CONN_DOWN, fg);
        }

        // Back across the gap row above the destination, then down into it.
        self.add_connection(r.lane_x, r.entry_y, CONN_DOWN | CONN_LEFT, fg);
        for x in (r.entry_x + 1)..r.lane_x {
            self.add_connection(x, r.entry_y, CONN_LEFT | CONN_RIGHT, fg);
        }
        self.add_connection(r.entry_x, r.entry_y, CONN_RIGHT | CONN_DOWN, fg);
        let arrow_y = r.dst_top_y.saturating_sub(1);
        for y in (r.entry_y + 1)..arrow_y {
            self.add_connection(r.entry_x, y, CONN_UP | CONN_DOWN, fg);
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
    /// Gap columns never contain nodes, so the route cannot pass through a
    /// node stacked above or below either endpoint.
    pub(crate) fn draw_feedback_edge_lr(&mut self, route: &FeedbackRouteLr, fg: Option<Color>) {
        let r = route;
        let exit_y = r.src_bottom_y + 1;
        let entry_y = r.dst_bottom_y + 2;

        // Leave the source: turn under its bottom border, run right, drop.
        self.add_connection(r.exit_x, exit_y, CONN_UP | CONN_RIGHT, fg);
        for x in (r.exit_x + 1)..r.exit_lane_x {
            self.add_connection(x, exit_y, CONN_LEFT | CONN_RIGHT, fg);
        }
        self.add_connection(r.exit_lane_x, exit_y, CONN_LEFT | CONN_DOWN, fg);
        for y in (exit_y + 1)..r.lane_y {
            self.add_connection(r.exit_lane_x, y, CONN_UP | CONN_DOWN, fg);
        }

        // Along the lane.
        self.add_connection(r.exit_lane_x, r.lane_y, CONN_UP | CONN_LEFT, fg);
        for x in (r.entry_lane_x + 1)..r.exit_lane_x {
            self.add_connection(x, r.lane_y, CONN_LEFT | CONN_RIGHT, fg);
        }
        self.add_connection(r.entry_lane_x, r.lane_y, CONN_UP | CONN_RIGHT, fg);

        // Rise beside the destination, run right under it, then up into it.
        for y in (entry_y + 1)..r.lane_y {
            self.add_connection(r.entry_lane_x, y, CONN_UP | CONN_DOWN, fg);
        }
        self.add_connection(r.entry_lane_x, entry_y, CONN_DOWN | CONN_RIGHT, fg);
        for x in (r.entry_lane_x + 1)..r.entry_x {
            self.add_connection(x, entry_y, CONN_LEFT | CONN_RIGHT, fg);
        }
        self.add_connection(r.entry_x, entry_y, CONN_LEFT | CONN_UP, fg);
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
    let plans = plan_feedback(graph, &layout);

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
    let core_height = layers.len() * (node_height + edge_gap) - edge_gap;
    let last_layer = layers.len() - 1;

    // A feedback edge leaves its source below the box and enters its
    // destination above the box. The source nearest the gutter turns on the
    // second gap row, which keeps its route clear of a straight-edge label on
    // the first; every other source turns on the first gap row and passes
    // under its right-hand siblings. Entries mirror this above the destination.
    // Two sources (or two targets) in one layer therefore never share a row.
    let exit_rows_below = |src_rank: usize| if src_rank == 0 { 2 } else { 1 };
    let entry_rows_above = |dst_rank: usize| if dst_rank == 0 { 3 } else { 4 };

    // Routes into the first layer or out of the last one need extra rows.
    let mut top_margin = 0usize;
    let mut bottom_margin = 0usize;
    for plan in &plans {
        let edge = &graph.edges[plan.edge];
        if let Some(&(layer, _)) = layout.node_pos.get(&edge.to)
            && layer == 0
        {
            top_margin = top_margin.max(entry_rows_above(plan.dst_rank));
        }
        if let Some(&(layer, _)) = layout.node_pos.get(&edge.from)
            && layer == last_layer
        {
            bottom_margin = bottom_margin.max(exit_rows_below(plan.src_rank));
        }
    }

    let max_label_width = plans
        .iter()
        .filter_map(|plan| graph.edges[plan.edge].label.as_ref())
        .map(|label| label.chars().count())
        .max()
        .unwrap_or(0);
    // Lanes sit four columns apart, plus room for a label beside each lane.
    let lane_gap = 4 + max_label_width;
    let gutter_width = if plans.is_empty() {
        0
    } else {
        lane_gap * plans.len()
    };
    let canvas_width = core_width + gutter_width;
    let canvas_height = top_margin + core_height + bottom_margin;

    let mut canvas = Canvas::new(canvas_width, canvas_height);

    // Calculate node positions and draw nodes
    let mut positions: HashMap<String, NodeLayout> = HashMap::new();
    let border_fg = Some(theme.code_border);
    let text_fg = Some(theme.fg);

    // First pass: calculate centers for the widest layer
    // Then align single-node layers to the canvas center
    let canvas_center = core_width / 2;

    for (layer_idx, layer) in layers.iter().enumerate() {
        let y = top_margin + layer_idx * (node_height + edge_gap);

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

    // Feedback edges go first so that forward-edge arrowheads and labels,
    // which overwrite cells, end up on top of any route they touch.
    let lane_x = |lane: usize| core_width + 1 + lane * lane_gap;
    let mut labels: Vec<(usize, usize, &str)> = Vec::new();
    for plan in &plans {
        let edge = &graph.edges[plan.edge];
        let (Some(src), Some(dst)) = (positions.get(&edge.from), positions.get(&edge.to)) else {
            continue;
        };
        let route = FeedbackRouteTd {
            exit_x: src.right_x().saturating_sub(1),
            src_bottom_y: src.bottom_y(),
            exit_y: src.bottom_y() + exit_rows_below(plan.src_rank),
            entry_x: dst.right_x().saturating_sub(1),
            dst_top_y: dst.top_y,
            entry_y: dst.top_y.saturating_sub(entry_rows_above(plan.dst_rank)),
            lane_x: lane_x(plan.lane),
        };
        canvas.draw_feedback_edge_td(&route, edge_fg);

        if let Some(text) = edge.label.as_deref() {
            // Beside this edge's lane, level with the middle of its vertical run.
            let y = (route.entry_y + route.exit_y) / 2;
            labels.push((route.lane_x + 2, y, text));
        }
    }
    for (x, y, text) in labels {
        for (i, ch) in text.chars().enumerate() {
            canvas.set(x + i, y, ch, label_fg);
        }
    }

    // Forward edges
    let feedback_set: HashSet<usize> = layout.feedback.iter().copied().collect();
    for (idx, edge) in graph.edges.iter().enumerate() {
        if feedback_set.contains(&idx) {
            continue;
        }
        if let (Some(src), Some(dst)) = (positions.get(&edge.from), positions.get(&edge.to)) {
            canvas.draw_edge_td(
                src.center_x,
                src.bottom_y(),
                dst.center_x,
                dst.top_y,
                edge.label.as_deref(),
                edge_fg,
                label_fg,
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
    let plans = plan_feedback(graph, &layout);

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

    let canvas_width: usize =
        col_widths.iter().sum::<usize>() + (layers.len().saturating_sub(1)) * node_h_gap + 4;
    let core_height = max_nodes_in_layer * (node_height + v_gap) - v_gap + 2;
    // One spare row under the diagram, then a lane every `lane_gap` rows.
    let gutter_height = if plans.is_empty() {
        0
    } else {
        lane_gap * plans.len() + 1
    };
    let canvas_height = core_height + gutter_height;

    let mut canvas = Canvas::new(canvas_width, canvas_height);

    let mut positions: HashMap<String, NodeLayout> = HashMap::new();
    let border_fg = Some(theme.code_border);
    let text_fg = Some(theme.fg);

    // (left, right) border columns of each layer's column.
    let mut col_bounds: Vec<(usize, usize)> = Vec::with_capacity(layers.len());
    let mut col_x = 2; // starting x with margin
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

        col_x += col_w + node_h_gap;
    }

    let edge_fg = Some(theme.code_border);
    let label_fg = Some(theme.h3);

    // Feedback edges go first so that forward-edge arrowheads and labels,
    // which overwrite cells, end up on top of any route they touch.
    let lane_y = |lane: usize| core_height + 1 + lane * lane_gap;
    let mut labels: Vec<(usize, usize, &str)> = Vec::new();
    for plan in &plans {
        let edge = &graph.edges[plan.edge];
        let (Some(src), Some(dst)) = (positions.get(&edge.from), positions.get(&edge.to)) else {
            continue;
        };
        // The source nearest the gutter drops one column clear of its border
        // and any other source two columns, so two sources stacked in one
        // column use different gap columns and an upper source's drop does
        // not hug the box below it.
        let route = FeedbackRouteLr {
            exit_x: src.right_x().saturating_sub(1),
            src_bottom_y: src.bottom_y(),
            exit_lane_x: src.right_x() + if plan.src_rank == 0 { 1 } else { 2 },
            entry_x: dst.left_x() + 1,
            dst_bottom_y: dst.bottom_y(),
            entry_lane_x: dst.left_x().saturating_sub(2),
            lane_y: lane_y(plan.lane),
        };
        canvas.draw_feedback_edge_lr(&route, edge_fg);

        if let Some(text) = edge.label.as_deref() {
            // Inline on the lane, centered on its horizontal run.
            let inner = route.exit_lane_x.saturating_sub(route.entry_lane_x + 1);
            let x = route.entry_lane_x + 1 + inner.saturating_sub(text.chars().count()) / 2;
            labels.push((x, route.lane_y, text));
        }
    }
    for (x, y, text) in labels {
        for (i, ch) in text.chars().enumerate() {
            canvas.set(x + i, y, ch, label_fg);
        }
    }

    // Forward edges
    let feedback_set: HashSet<usize> = layout.feedback.iter().copied().collect();
    for (idx, edge) in graph.edges.iter().enumerate() {
        if feedback_set.contains(&idx) {
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
        rx.recv_timeout(Duration::from_secs(10))
            .expect("rendering did not finish within 10 seconds")
    }

    /// Compare a render against a snapshot written at column 0 inside a raw
    /// string literal (one leading newline, trailing blank rows ignored).
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

    // ── Cycles ──

    #[test]
    fn top_down_cycle_wraps_around_the_right() {
        assert_render(
            "graph TB\n    Loop --> Execute\n    Execute --> Repeat\n    Repeat --> Loop\n",
            r#"
          ┌───────┐
          │       │
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
           │      │
           └──────┘
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
┌──┘                    │
└───────────────────────┘
"#,
        );
    }

    #[test]
    fn self_loop_top_down_is_a_closed_loop() {
        assert_render(
            "graph TD\n    A[Self] --> A\n",
            r#"
         ┌─────┐
         │     │
         ▼     │
   ┌──────┐    │
   │ Self │    │
   └──────┘    │
         │     │
         └─────┘
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
┌──┘      │
└─────────┘
"#,
        );
    }

    // ── Routes must not pass through other nodes ──

    #[test]
    fn feedback_edge_avoids_sibling_nodes_top_down() {
        let code = "graph TD\n    A --> B\n    A --> C\n    B --> D\n    C --> D\n    D --> B\n";
        let text = render_text(code);
        // Nothing may be drawn between the two siblings on their middle row.
        assert!(text.contains("│  B  │    │  C  │"), "{text}");
        assert_render(
            code,
            r#"
         ┌─────┐
         │  A  │
         └─────┘
            │
        ┌───┼────────────┐
      ┌─┼───┴────┐       │
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
              │          │
              └──────────┘
"#,
        );
    }

    #[test]
    fn feedback_edge_avoids_stacked_nodes_left_right() {
        let code = "graph LR\n    A --> B\n    A --> C\n    B --> D\n    C --> D\n    D --> B\n";
        let text = render_text(code);
        // C sits directly below B; the route into B must not cross C's box.
        let c_col = text
            .lines()
            .find_map(|line| line.find("│  C  │"))
            .expect("C is drawn");
        for line in text.lines() {
            let cell = line.chars().nth(c_col + 3).unwrap_or(' ');
            assert!(
                cell != '│' || line.contains("│  B  │") || line.contains("│  C  │"),
                "line through the column of C:\n{text}"
            );
        }
        assert_render(
            code,
            r#"

               ┌─────┐
            ┌─▶│  B  │───┐
  ┌─────┐   │  └─────┘   │  ┌─────┐
  │  A  │───┤   ▲        ├─▶│  D  │
  └─────┘   │┌──┘        │  └─────┘
            ││ ┌─────┐   │       └─┐
            └┼▶│  C  │───┘         │
             │ └─────┘             │
             │                     │
             │                     │
             └─────────────────────┘
"#,
        );
    }

    #[test]
    fn two_feedback_sources_in_one_layer_leave_on_different_rows() {
        assert_render(
            "graph TD\n    S --> A\n    S --> B\n    A --> S\n    B --> S\n",
            r#"
              ┌──────────┬───┐
              │          │   │
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
        └──────────┼─────┘   │
                   └─────────┘
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
┌──┘        └─▶│  B  │ │
│              └─────┘ │
│                   └─┐│
│                     ││
├─────────────────────┼┘
│                     │
└─────────────────────┘
"#,
        );
    }

    // ── Labels ──

    #[test]
    fn feedback_labels_sit_beside_their_own_lane_top_down() {
        let code = "graph TD\n    Start --> Check\n    Check --> Work\n    Work --> Done\n    Done -->|retry| Start\n    Work -->|again| Check\n";
        let text = render_text(code);
        assert!(text.contains("again"), "{text}");
        assert!(text.contains("retry"), "{text}");
        assert_render(
            code,
            r#"
          ┌──────────────┐
          │              │
          ▼              │
   ┌───────┐             │
   │ Start │             │
   └───────┘             │
       │                 │
       │  ┌─────┐        │
       │  │     │        │
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
       │ │      │        │
       │ └──────┘        │
       │                 │
       ▼                 │
   ┌──────┐              │
   │ Done │              │
   └──────┘              │
         │               │
         └───────────────┘
"#,
        );
    }

    #[test]
    fn feedback_labels_sit_inline_on_their_lane_left_right() {
        let code = "graph LR\n    Start --> Check\n    Check --> Work\n    Work --> Done\n    Done -->|retry| Start\n    Work -->|again| Check\n";
        let text = render_text(code);
        assert!(text.contains("again"), "{text}");
        assert!(text.contains("retry"), "{text}");
        assert_render(
            code,
            r#"

  ┌───────┐      ┌───────┐      ┌──────┐      ┌──────┐
  │ Start │─────▶│ Check │─────▶│ Work │─────▶│ Done │
  └───────┘      └───────┘      └──────┘      └──────┘
   ▲              ▲                   └─┐           └─┐
┌──┘           ┌──┘                     │             │
│              └─────────again──────────┘             │
│                                                     │
└────────────────────────retry────────────────────────┘
"#,
        );
    }
}
