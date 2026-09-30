//! タイル分割。1つの階の部材を平面の四分木に分け、大きい部材ほど上位のタイルに置く（ADD）。
//!
//! 各ノードは、ノードの平面の広がりの1/4以上の部材を（最大`max_features`個まで）自分で持ち、
//! 残りを重心で4つに分ける。ノードのgeometricErrorは、子孫に回した部材の対角長の最大値とする。
//! 遠くからは大きな部材だけが描かれ、近づくと小さな部材が足される。

/// 軸平行の外接箱。
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

    /// 点列の外接箱。空なら`EMPTY`。
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

/// タイルの木のノード。
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// 四分木のパス（根は`r`、子は`r0`〜`r3`…）。
    pub path: String,
    /// このノードのcontentに入れる部材（呼び出し側の番号）。
    pub elements: Vec<usize>,
    pub geometric_error: f64,
    /// このノードと子孫の全部材の外接箱。
    pub bounds: Aabb,
    pub children: Vec<Node>,
}

const MAX_DEPTH: usize = 10;

/// `items`は（部材の番号、外接箱）。
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
    // 大きい順に並べ、しきい値以上の部材を上限まで自分で持つ
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
        // 分けても同じ集合になる（重心がすべて同じ象限）
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
            // 子のgeometricErrorは親以下（SSEが単調になる）
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
        // 100 m四方に、40 mの床2枚と1 mの家具400個
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
