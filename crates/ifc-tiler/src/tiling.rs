//! Tile splitting. The elements of one storey are divided by a planar quadtree, and larger elements go in higher tiles (ADD).
//!
//! Each node keeps the elements whose diagonal is at least 1/4 of the node's planar extent (up to `max_features`),
//! and splits the rest into four by centroid. The node's geometricError is the largest diagonal among the elements pushed to descendants.
//! From far away only large elements are drawn, and smaller ones are added as the camera approaches.

/// Axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Aabb {
    pub const EMPTY: Self = Self { min: [f64::INFINITY; 3], max: [f64::NEG_INFINITY; 3] };

    pub fn add(&mut self, p: [f64; 3]) {
        for (i, v) in p.into_iter().enumerate() {
            self.min[i] = self.min[i].min(v);
            self.max[i] = self.max[i].max(v);
        }
    }

    /// Bounding box of a sequence of points. `EMPTY` if there are none.
    pub fn from_points(points: impl IntoIterator<Item = [f64; 3]>) -> Self {
        let mut b = Self::EMPTY;
        points.into_iter().for_each(|p| b.add(p));
        b
    }

    pub fn union(mut self, o: &Self) -> Self {
        self.add(o.min);
        self.add(o.max);
        self
    }

    pub fn center(&self) -> [f64; 3] {
        std::array::from_fn(|i| 0.5 * (self.min[i] + self.max[i]))
    }

    pub fn size(&self) -> [f64; 3] {
        std::array::from_fn(|i| self.max[i] - self.min[i])
    }

    pub fn diagonal(&self) -> f64 {
        self.size().iter().map(|d| d * d).sum::<f64>().sqrt()
    }
}

/// A node of the tile tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// Quadtree path (`r` for the root, `r0`–`r3`… for children).
    pub path: String,
    /// Elements placed in this node's content (indices of the caller).
    pub elements: Vec<usize>,
    pub geometric_error: f64,
    /// Bounding box of all elements of this node and its descendants.
    pub bounds: Aabb,
    pub children: Vec<Node>,
}

const MAX_DEPTH: usize = 10;

/// `items` are (element index, bounding box).
pub fn build(items: &[(usize, Aabb)], max_features: usize) -> Node {
    node(items.to_vec(), "r".into(), 0, max_features.max(1))
}

fn node(mut items: Vec<(usize, Aabb)>, path: String, depth: usize, max: usize) -> Node {
    let bounds = items.iter().fold(Aabb::EMPTY, |b, (_, a)| b.union(a));
    let leaf = |items: Vec<(usize, Aabb)>, path| Node {
        path,
        elements: items.into_iter().map(|(i, _)| i).collect(),
        geometric_error: 0.0,
        bounds,
        children: Vec::new(),
    };
    if items.len() <= max || depth == MAX_DEPTH {
        return leaf(items, path);
    }
    let [sx, sy, _] = bounds.size();
    let threshold = sx.max(sy) / 4.0;
    // Sort by size, and keep the elements above the threshold up to the limit
    items.sort_by(|a, b| b.1.diagonal().total_cmp(&a.1.diagonal()).then(a.0.cmp(&b.0)));
    let own = items.iter().take(max).take_while(|(_, a)| a.diagonal() >= threshold).count();
    let rest = items.split_off(own);
    let c = bounds.center();
    let mut quadrants: [Vec<(usize, Aabb)>; 4] = Default::default();
    for it in rest {
        let [x, y, _] = it.1.center();
        quadrants[usize::from(x > c[0]) + 2 * usize::from(y > c[1])].push(it);
    }
    if own == 0 && quadrants.iter().filter(|q| !q.is_empty()).count() == 1 {
        // Splitting would give the same set (all centroids fall in the same quadrant)
        return leaf(quadrants.into_iter().flatten().collect(), path);
    }
    let geometric_error = quadrants.iter().flatten().map(|(_, a)| a.diagonal()).fold(0.0, f64::max);
    let children = quadrants
        .into_iter()
        .enumerate()
        .filter(|(_, q)| !q.is_empty())
        .map(|(k, q)| node(q, format!("{path}{k}"), depth + 1, max))
        .collect();
    Node { path, elements: items.into_iter().map(|(i, _)| i).collect(), geometric_error, bounds, children }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(x: f64, y: f64, size: f64) -> Aabb {
        Aabb { min: [x, y, 0.0], max: [x + size, y + size, size] }
    }

    fn all(n: &Node) -> Vec<usize> {
        let mut v = n.elements.clone();
        for c in &n.children {
            v.extend(all(c));
        }
        v.sort();
        v
    }

    fn check_invariants(n: &Node, max: usize) {
        assert!(n.elements.len() <= max || n.children.is_empty(), "{}", n.path);
        for c in &n.children {
            // A child's geometricError is at most its parent's (keeps SSE monotonic)
            assert!(c.geometric_error <= n.geometric_error, "{} > {}", c.path, n.path);
            check_invariants(c, max);
        }
        if n.children.is_empty() {
            assert_eq!(n.geometric_error, 0.0);
        }
    }

    #[test]
    fn small_sets_are_leaves() {
        let items: Vec<_> = (0..5).map(|i| (i, cube(i as f64, 0.0, 1.0))).collect();
        let n = build(&items, 5);
        assert!(n.children.is_empty());
        assert_eq!(n.elements.len(), 5);
        assert_eq!(n.geometric_error, 0.0);
    }

    #[test]
    fn large_elements_stay_on_top_and_small_ones_go_down() {
        // Within 100 m × 100 m: two 40 m floors and 400 pieces of 1 m furniture
        let mut items = vec![(0, cube(0.0, 0.0, 40.0)), (1, cube(60.0, 60.0, 40.0))];
        for i in 0..400 {
            items.push((2 + i, cube((i % 20) as f64 * 5.0, (i / 20) as f64 * 5.0, 1.0)));
        }
        let n = build(&items, 100);
        assert_eq!(n.elements, vec![0, 1]);
        assert!((n.geometric_error - 3f64.sqrt()).abs() < 1e-9);
        assert_eq!(all(&n), (0..402).collect::<Vec<_>>());
        check_invariants(&n, 100);
        assert!(n.children.iter().all(|c| c.path.len() == 2));
    }

    #[test]
    fn identical_centroids_terminate() {
        let items: Vec<_> = (0..50).map(|i| (i, cube(0.0, 0.0, 0.1))).collect();
        let n = build(&items, 10);
        assert_eq!(all(&n).len(), 50);
    }

    #[test]
    fn aabb_basics() {
        let mut b = Aabb::EMPTY;
        b.add([0.0, 0.0, 0.0]);
        b.add([3.0, 4.0, 0.0]);
        assert_eq!(b.diagonal(), 5.0);
        assert_eq!(b.center(), [1.5, 2.0, 0.0]);
    }
}
