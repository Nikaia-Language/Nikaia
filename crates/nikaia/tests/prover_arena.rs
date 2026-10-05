//! **The prover's terms held in Nikaia** (`tools/prover_arena.nika`, #436):
//! every operation answers what `nikaia-logic`'s `Arena` answers on the same
//! terms, built in the same order.

use std::collections::{BTreeMap, BTreeSet};

use nikaia_logic::{Arena, TermId};
use nikaia_std::tools::prover_arena::TermArena;

/// The same terms in both arenas: `0 <= i`, `i < n + 1`, `2 * n - x == 7`,
/// `!(i > 3) || x != i`, `-(4 * 5)`, and their conjunction.
fn both() -> (Arena, Vec<TermId>, TermArena, Vec<i64>) {
    let mut a = Arena::new();
    let mut b = TermArena::empty();
    let (i, n, x) = (a.var("i"), a.var("n"), a.var("x"));
    let (bi, bn, bx) = (b.var("i"), b.var("n"), b.var("x"));
    let (zero, one, two, three, seven) = (a.int(0), a.int(1), a.int(2), a.int(3), a.int(7));
    let (bzero, bone, btwo, bthree, bseven) = (b.int(0), b.int(1), b.int(2), b.int(3), b.int(7));
    let f1 = a.le(zero, i);
    let g1 = b.le(bzero, bi);
    let np1 = a.add(n, one);
    let bnp1 = b.add(bn, bone);
    let f2 = a.lt(i, np1);
    let g2 = b.lt(bi, bnp1);
    let tn = a.mul(two, n);
    let btn = b.mul(btwo, bn);
    let d = a.sub(tn, x);
    let bd = b.sub(btn, bx);
    let f3 = a.eq(d, seven);
    let g3 = b.equal(bd, bseven);
    let gt = a.gt(i, three);
    let bgt = b.gt(bi, bthree);
    let ngt = a.not(gt);
    let bngt = b.not(bgt);
    let ne = a.ne(x, i);
    let bne = b.unequal(bx, bi);
    let f4 = a.or(vec![ngt, ne]);
    let g4 = b.or(vec![bngt, bne]);
    let (four, five) = (a.int(4), a.int(5));
    let (bfour, bfive) = (b.int(4), b.int(5));
    let p = a.mul(four, five);
    let bp = b.mul(bfour, bfive);
    let f5 = a.neg(p);
    let g5 = b.neg(bp);
    let all = a.and(vec![f1, f2, f3, f4]);
    let ball = b.and(vec![g1, g2, g3, g4]);
    (
        a,
        vec![f1, f2, f3, f4, f5, all, d, np1],
        b,
        vec![g1, g2, g3, g4, g5, ball, bd, bnp1],
    )
}

#[test]
fn every_operation_answers_as_nikaia_logic_does() {
    let (mut a, ids, mut b, bids) = both();
    for (id, bid) in ids.iter().zip(&bids) {
        for name in ["i", "n", "x", "y"] {
            assert_eq!(
                a.mentions(*id, name),
                b.mentions(*bid, name),
                "mentions {name}"
            );
        }
        let mut va = BTreeSet::new();
        a.variables(*id, &mut va);
        let mut vb = BTreeSet::new();
        b.variables(*bid, &mut vb);
        assert_eq!(va, vb);
        assert_eq!(a.constant(*id), b.constant(*bid));
        let values = BTreeMap::from([
            ("i".to_string(), 2),
            ("n".to_string(), 5),
            ("x".to_string(), 3),
        ]);
        assert_eq!(a.int_value(*id, &values), b.int_value(*bid, &values));
        assert_eq!(a.size(*id) as i64, b.size(*bid));
    }
    // Substitution builds the same nodes, in the same order.
    let (n_a, x_a) = (a.var("n"), a.var("x"));
    let (n_b, x_b) = (b.var("n"), b.var("x"));
    let with_a = BTreeMap::from([("i".to_string(), n_a), ("n".to_string(), x_a)]);
    let with_b = BTreeMap::from([("i".to_string(), n_b), ("n".to_string(), x_b)]);
    for (id, bid) in ids.iter().zip(&bids) {
        let sa = a.substitute(*id, &with_a);
        let sb = b.substitute(*bid, &with_b);
        assert_eq!(sa.index() as i64, sb, "the same place");
        let mut va = BTreeSet::new();
        a.variables(sa, &mut va);
        let mut vb = BTreeSet::new();
        b.variables(sb, &mut vb);
        assert_eq!(va, vb);
    }
    assert_eq!(a.len() as i64, b.nodes.len() as i64);
}
