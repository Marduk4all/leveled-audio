use rayon::prelude::*;

/// Führt `f` für alle Einträge parallel aus (`jobs` Threads) und gibt die
/// Ergebnisse **in Eingabereihenfolge** zurück, unabhängig davon, welche
/// Datei zuerst fertig wird.
pub fn run_batch<T, R, F>(items: Vec<T>, jobs: usize, f: F) -> Vec<R>
where
    T: Send,
    R: Send,
    F: Fn(usize, T) -> R + Sync + Send,
{
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs.max(1))
        .thread_name(|i| format!("leveled-audio-worker-{i}"))
        .build()
        .expect("Thread-Pool anlegen");
    pool.install(|| {
        items
            .into_par_iter()
            .enumerate()
            .map(|(i, item)| f(i, item))
            .collect()
    })
}
