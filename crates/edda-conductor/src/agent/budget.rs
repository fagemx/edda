/// Tracks cumulative cost across phases and enforces plan-level budget.
pub struct BudgetTracker {
    plan_budget: Option<f64>,
    spent: f64,
}

impl BudgetTracker {
    pub fn new(plan_budget: Option<f64>) -> Self {
        Self {
            plan_budget,
            spent: 0.0,
        }
    }

    pub fn record(&mut self, phase_cost: f64) {
        self.spent += phase_cost;
    }

    pub fn spent(&self) -> f64 {
        self.spent
    }

    pub fn remaining(&self) -> Option<f64> {
        self.plan_budget.map(|b| (b - self.spent).max(0.0))
    }

    pub fn is_exhausted(&self) -> bool {
        self.plan_budget.map(|b| self.spent >= b).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_budget_never_exhausted() {
        let mut t = BudgetTracker::new(None);
        t.record(100.0);
        assert!(!t.is_exhausted());
        assert_eq!(t.remaining(), None);
    }

    #[test]
    fn tracks_spending() {
        let mut t = BudgetTracker::new(Some(10.0));
        assert!(!t.is_exhausted());
        assert!((t.remaining().unwrap() - 10.0).abs() < 0.001);

        t.record(3.0);
        assert!(!t.is_exhausted());
        assert!((t.remaining().unwrap() - 7.0).abs() < 0.001);
        assert!((t.spent() - 3.0).abs() < 0.001);
    }

    #[test]
    fn exhausted_at_budget() {
        let mut t = BudgetTracker::new(Some(5.0));
        t.record(5.0);
        assert!(t.is_exhausted());
        assert!((t.remaining().unwrap()).abs() < 0.001);
    }

    #[test]
    fn exhausted_over_budget() {
        let mut t = BudgetTracker::new(Some(5.0));
        t.record(7.0);
        assert!(t.is_exhausted());
        assert!((t.remaining().unwrap()).abs() < 0.001); // clamped to 0
    }
}
