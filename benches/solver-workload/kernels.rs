// benches/solver-workload/kernels.rs
//
// The hand-written Rust half of ADR-270 D8 step 1: the three kernels of
// `benches/solver-kernels.nika`, the same algorithms, the same allocations and
// the same checksums, written as a Rust programmer writes them - slices for
// what is only read, `&mut Vec` for what is filled. Measured beside the
// lowering by `crates/nikaia/tests/measure.rs`.

fn next(state: u64) -> u64 {
    state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407)
}

#[allow(clippy::too_many_arguments)]
fn combine(
    rc: &[i64],
    rv: &[i64],
    a: i64,
    sc: &[i64],
    sv: &[i64],
    b: i64,
    oc: &mut Vec<i64>,
    ov: &mut Vec<i64>,
) -> i64 {
    let limit: i64 = 2147483647;
    let mut promoted = 0;
    let (mut i, mut j) = (0, 0);
    while i < rc.len() || j < sc.len() {
        let (col, value);
        if j >= sc.len() || (i < rc.len() && rc[i] < sc[j]) {
            col = rc[i];
            value = a * rv[i];
            i += 1;
        } else if i >= rc.len() || sc[j] < rc[i] {
            col = sc[j];
            value = b * sv[j];
            j += 1;
        } else {
            col = rc[i];
            value = a * rv[i] + b * sv[j];
            i += 1;
            j += 1;
        }
        if value != 0 {
            if value > limit || value < -limit {
                promoted += 1;
            }
            oc.push(col);
            ov.push(value);
        }
    }
    promoted
}

fn rows(n: i64) -> i64 {
    let (width, columns, count) = (16i64, 4096i64, 256i64);
    let mut state: u64 = 7;
    let mut cols: Vec<Vec<i64>> = Vec::new();
    let mut vals: Vec<Vec<i64>> = Vec::new();
    for _ in 0..count {
        let mut c = Vec::new();
        let mut v = Vec::new();
        let mut at = 0i64;
        for _ in 0..width {
            state = next(state);
            at += 1 + ((state >> 33) % (columns / width) as u64) as i64;
            c.push(at);
            state = next(state);
            v.push(((state >> 40) % 2001) as i64 - 1000);
        }
        cols.push(c);
        vals.push(v);
    }
    let mut checksum = 0;
    for round in 0..n {
        let r = (round % count) as usize;
        let s = ((round * 7 + 1) % count) as usize;
        let mut oc = Vec::new();
        let mut ov = Vec::new();
        let promoted = combine(
            &cols[r],
            &vals[r],
            3 + round % 5,
            &cols[s],
            &vals[s],
            -2 - round % 3,
            &mut oc,
            &mut ov,
        );
        checksum += promoted + oc.len() as i64;
        if !oc.is_empty() {
            checksum += ov[0] % 97;
        }
    }
    checksum
}

fn watch(n: i64) -> i64 {
    let (variables, clauses, size) = (2000usize, 8000usize, 4usize);
    let mut state: u64 = 11;
    let mut arena: Vec<usize> = Vec::new();
    for _ in 0..clauses * size {
        state = next(state);
        arena.push(((state >> 33) % (2 * variables) as u64) as usize);
    }
    let mut watched: Vec<Vec<usize>> = vec![Vec::new(); 2 * variables];
    let mut blockers: Vec<Vec<usize>> = vec![Vec::new(); 2 * variables];
    for c in 0..clauses {
        let (first, second) = (arena[c * size], arena[c * size + 1]);
        watched[first].push(c);
        blockers[first].push(second);
        watched[second].push(c);
        blockers[second].push(first);
    }
    let mut value: Vec<i64> = vec![0; 2 * variables];
    let mut checksum: i64 = 0;
    for round in 0..n {
        for v in 0..variables {
            state = next(state);
            match (state >> 33) % 3 {
                0 => {
                    value[2 * v] = 1;
                    value[2 * v + 1] = -1;
                }
                1 => {
                    value[2 * v] = -1;
                    value[2 * v + 1] = 1;
                }
                _ => {
                    value[2 * v] = 0;
                    value[2 * v + 1] = 0;
                }
            }
        }
        for lit in 0..2 * variables {
            if value[lit] == -1 {
                let list = &watched[lit];
                let blocks = &blockers[lit];
                for k in 0..list.len() {
                    if value[blocks[k]] == 1 {
                        checksum += 1;
                    } else {
                        let start = list[k] * size;
                        let mut found: i64 = -1;
                        for at in 2..size {
                            let other = arena[start + at];
                            if found < 0 && value[other] != -1 {
                                found = other as i64;
                            }
                        }
                        checksum += if found >= 0 { 3 } else { 7 };
                    }
                }
            }
        }
        checksum = checksum % 1000000007 + round;
    }
    checksum
}

fn multiply(x: &[u32], y: &[u32]) -> Vec<u32> {
    let mut out = vec![0; x.len() + y.len()];
    for i in 0..x.len() {
        let mut carry: u64 = 0;
        let xi = x[i] as u64;
        for j in 0..y.len() {
            let t = xi * y[j] as u64 + out[i + j] as u64 + carry;
            out[i + j] = t as u32;
            carry = t >> 32;
        }
        out[i + y.len()] = carry as u32;
    }
    out
}

fn bignum(n: i64) -> i64 {
    let limbs = 64usize;
    let mut state: u64 = 13;
    let mut x: Vec<u32> = Vec::new();
    let mut y: Vec<u32> = Vec::new();
    for _ in 0..limbs {
        state = next(state);
        x.push((state >> 32) as u32);
        state = next(state);
        y.push((state >> 32) as u32);
    }
    let mut checksum: u64 = 0;
    for round in 0..n as usize {
        let out = multiply(&x, &y);
        checksum = checksum.wrapping_add(out[round % (2 * limbs)] as u64);
        x[round % limbs] = out[limbs];
    }
    (checksum % 1000000007) as i64
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: i64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1000);
    let which = args.get(2).map(String::as_str).unwrap_or("all");
    if which == "rows" || which == "all" {
        println!("rows {}", rows(n));
    }
    if which == "watch" || which == "all" {
        println!("watch {}", watch(n / 100 + 1));
    }
    if which == "bignum" || which == "all" {
        println!("bignum {}", bignum(n));
    }
}
