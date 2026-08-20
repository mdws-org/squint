import Foundation

/// Runs jobs with a concurrency limit chosen from what the machine can hold.
///
/// Cores are the wrong limit here. A perceptual comparison of a 12 megapixel
/// image peaks at 2.84 GB, so running one per core drives a 16 GB machine into
/// swap and finishes slower than running fewer. Fast mode evaluates no metric and
/// is bounded by cores as usual.
@MainActor
final class JobQueue: ObservableObject {
    /// One queue for the whole application. The window and the Finder service
    /// both feed it, so work arriving by either route joins the same run and
    /// respects the same concurrency limit — including work that arrives while a
    /// run is already under way, which is re-checked rather than left waiting.
    static let shared = JobQueue()

    private init() {}

    @Published private(set) var jobs: [Job] = []
    @Published private(set) var isRunning = false
    /// The mode the window's picker governs. It decides what a drop onto the
    /// window does, and nothing else: files arriving from Finder carry their own.
    @Published var mode: Engine.Mode = .fast
    @Published var target: Double = 80

    /// Measured peak for one 12 megapixel comparison: 2.84 GB. Rounded up,
    /// because the figure grows with pixel count and phone cameras are moving up.
    private static let bytesPerQualityJob: UInt64 = 3_000_000_000

    /// Fraction of physical memory this application is willing to claim.
    ///
    /// Chosen to reproduce the measured optimum. On an 8 core, 8 GB machine,
    /// 16 files in quality mode took 36 s at two concurrent, 60 s at four, and
    /// 116 s at eight, against 63 s if run one at a time. Concurrency past the
    /// point where the working sets fit is not merely wasted, it is negative:
    /// eight-way was nearly twice as slow as serial. This share yields 2 on 8 GB,
    /// 4 on 16 GB, 8 on 32 GB.
    private static let memoryShare = 0.75

    /// The cap for a batch, set by the most demanding mode it contains.
    ///
    /// A quality job holds gigabytes where a fast job holds megabytes, so one
    /// quality job in the batch binds the whole pass. Batches are single-mode in
    /// practice, since a right-click carries one instruction; the mixed case is
    /// rare enough to be worth handling conservatively rather than cleverly.
    static func concurrencyLimit(for jobs: [Job]) -> Int {
        let cores = ProcessInfo.processInfo.activeProcessorCount
        guard jobs.contains(where: { $0.mode == .quality }) else {
            // Fast peaks at 167 MB per job and strip does not decode at all, so
            // memory is not the binding constraint for either.
            return cores
        }
        let usable = UInt64(Double(ProcessInfo.processInfo.physicalMemory) * memoryShare)
        let byMemory = Int(usable / bytesPerQualityJob)
        return max(1, min(cores, byMemory))
    }

    /// Queue files, in the mode they were asked for.
    ///
    /// The mode travels with the job rather than being read off this object when
    /// the run starts. A right-click carries its own instruction, and a file
    /// queued as Strip must not be re-encoded because something else changed the
    /// window's picker in between.
    func add(_ urls: [URL], mode: Engine.Mode, target: Double? = nil) {
        let requested = target ?? self.target
        jobs.append(contentsOf: urls.map { Job(url: $0, mode: mode, target: requested) })

        // Claimed here, synchronously, rather than inside the task. `run` is
        // enqueued and not executed, so two calls landing before it starts would
        // both find this false and both begin a run over overlapping snapshots —
        // the same file read, optimized and replaced twice at once.
        guard !isRunning else { return }
        isRunning = true
        Task { await run() }
    }

    func clear() {
        guard !isRunning else { return }
        jobs.removeAll()
    }

    /// Drain the queue, including anything that arrives while draining.
    ///
    /// `isRunning` is already true on entry, claimed by `add`.
    private func run() async {
        defer { isRunning = false }

        // Re-checked after each pass. Work queued while a pass was in flight used
        // to be appended and then never looked at again: the rows sat at
        // "waiting" for good, which reads as done and is the worst possible
        // outcome for a batch whose whole purpose was removing a location.
        while true {
            let pass = jobs.filter { $0.state == .waiting }
            if pass.isEmpty { return }
            await runPass(pass)
        }
    }

    private func runPass(_ pass: [Job]) async {
        var pending = pass[...]
        let limit = Self.concurrencyLimit(for: pass)

        await withTaskGroup(of: Void.self) { group in
            var active = 0
            while !pending.isEmpty || active > 0 {
                while active < limit, let job = pending.first {
                    pending = pending.dropFirst()
                    active += 1
                    job.state = .working
                    let (mode, target) = (job.mode, job.target)
                    group.addTask { await Self.process(job, mode: mode, target: target) }
                }
                if active > 0 {
                    await group.next()
                    active -= 1
                }
            }
        }
    }

    /// Read, optimize, and replace. The work happens off the main actor; only the
    /// state update comes back to it.
    private static func process(_ job: Job, mode: Engine.Mode, target: Double) async {
        let url = await job.url
        let outcome: Job.State = await Task.detached(priority: .userInitiated) {
            do {
                let input = try Data(contentsOf: url)
                let result = try Engine.optimize(input, mode: mode, target: target)
                try Writer.replaceInPlace(url, with: result.data)
                return .done(
                    bytes: result.data.count,
                    originalBytes: result.originalBytes,
                    score: result.score,
                    hdr: result.hdr,
                    quantized: result.quantized
                )
            } catch let failure as Engine.Failure {
                return failure.isAlreadyOptimal ? .alreadyOptimal : .failed(failure.message)
            } catch {
                return .failed(error.localizedDescription)
            }
        }.value

        await MainActor.run { job.state = outcome }
    }
}
