import Foundation

/// Runs jobs with a concurrency limit chosen from what the machine can hold.
///
/// Cores are the wrong limit here. A perceptual comparison of a 12 megapixel
/// image peaks near 1.8 GB, so running one per core drives a 16 GB machine into
/// swap and finishes slower than running fewer. Fast mode evaluates no metric and
/// is bounded by cores as usual.
@MainActor
final class JobQueue: ObservableObject {
    @Published private(set) var jobs: [Job] = []
    @Published private(set) var isRunning = false
    @Published var mode: Engine.Mode = .fast
    @Published var target: Double = 80

    private static let bytesPerComparison: UInt64 = 2_000_000_000

    var concurrencyLimit: Int {
        let cores = ProcessInfo.processInfo.activeProcessorCount
        switch mode {
        case .fast:
            return cores
        case .quality:
            let memory = ProcessInfo.processInfo.physicalMemory
            let byMemory = Int(memory / Self.bytesPerComparison)
            return max(1, min(cores, byMemory))
        }
    }

    func add(_ urls: [URL]) {
        let new = urls.map(Job.init(url:))
        jobs.append(contentsOf: new)
        guard !isRunning else { return }
        Task { await run() }
    }

    func clear() {
        guard !isRunning else { return }
        jobs.removeAll()
    }

    private func run() async {
        isRunning = true
        defer { isRunning = false }

        let limit = concurrencyLimit
        let mode = self.mode
        let target = self.target

        var pending = jobs.filter { $0.state == .waiting }[...]

        await withTaskGroup(of: Void.self) { group in
            var active = 0
            while !pending.isEmpty || active > 0 {
                while active < limit, let job = pending.first {
                    pending = pending.dropFirst()
                    active += 1
                    job.state = .working
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
                    score: result.score
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
