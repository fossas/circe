use itertools::Itertools;

/// Implements `priority_find` for collections.
pub trait PriorityFind<T> {
    /// Searches through a list of items using the provided prioritization function.
    /// Priorities are such that a lower number is higher priority, meaning that `0` is the highest possible priority.
    ///
    /// As the search is performed:
    /// - If an item with the lowest priority is found, it is immediately returned and the rest of the search is aborted.
    /// - Otherwise, the highest priority item found is retained until the end of the search, at which point it is returned.
    fn priority_find<F: Fn(&T) -> usize>(self, prioritize: F) -> Option<T>;
}

impl<T, I> PriorityFind<T> for I
where
    I: Iterator<Item = T>,
{
    fn priority_find<F: Fn(&T) -> usize>(self, prioritize: F) -> Option<T> {
        priority_find(self, prioritize)
    }
}

/// Searches through a list of items using a priority function returning a non-negative number.
/// Priorities are such that a lower number is higher priority, meaning that `0` is the highest possible priority.
///
/// As the search is performed:
/// - If an item with the lowest priority is found, it is immediately returned and the rest of the search is aborted.
/// - Otherwise, the highest priority item found is retained until the end of the search, at which point it is returned.
fn priority_find<T, F: Fn(&T) -> usize>(
    items: impl IntoIterator<Item = T>,
    prioritize: F,
) -> Option<T> {
    items
        .into_iter()
        // Mapping here allows the function to use `take_while_inclusive` to bound the search below
        // instead of using more complex logic in `fold`.
        .map(|item| (prioritize(&item), item))
        // This ensures that the fold stops after finding the first priority 0 item, which constitutes an early termination condition.
        // Any item that isn't at priority 0 doesn't allow the function to early return: it might find a higher priority item later.
        .take_while_inclusive(|(priority, _)| *priority > 0)
        // The job of fold is now simple: just always select the item with higher priority.
        .fold(None, |result, (incoming, item)| {
            match result {
                // No result yet, so incoming item is automatically highest priority.
                None => Some((incoming, item)),

                // Remember that "lower number" means "higher priority".
                // If the new item isn't higher priority, keep the current pick:
                // this ensures the first item encountered at a given priority is chosen.
                Some((current, _)) => {
                    if current > incoming {
                        Some((incoming, item))
                    } else {
                        result
                    }
                }
            }
        })
        .map(|(_, item)| item)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_find_basic() {
        let items = vec!["a", "b", "c", "d", "e"];
        let result = priority_find(items, |item| match *item {
            "a" => 2,
            "b" => 1,
            "d" => 0,
            _ => 3,
        });
        assert_eq!(result, Some("d"));
    }

    #[test]
    fn priority_find_all_zero() {
        let items = vec!["a", "b", "c", "d", "e"];
        let result = priority_find(items, |_| 0);
        assert_eq!(result, Some("a"));
    }

    #[test]
    fn priority_find_empty() {
        let items = Vec::<&str>::new();
        let result = priority_find(items, |_| 0);
        assert_eq!(result, None);
    }
}
