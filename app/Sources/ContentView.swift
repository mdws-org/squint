import SwiftUI
import UniformTypeIdentifiers

struct ContentView: View {
    @StateObject private var queue = JobQueue.shared
    @State private var isTargeted = false

    var body: some View {
        VStack(spacing: 0) {
            controls
            Divider()
            if queue.jobs.isEmpty {
                dropPrompt
            } else {
                list
            }
        }
        .frame(minWidth: 460, minHeight: 320)
        .onDrop(of: [.fileURL], isTargeted: $isTargeted) { providers in
            load(providers)
            return true
        }
        .background(isTargeted ? Color.accentColor.opacity(0.08) : Color.clear)
    }

    private var controls: some View {
        HStack(spacing: 12) {
            Picker("", selection: $queue.mode) {
                Text("Fast").tag(Engine.Mode.fast)
                Text("Quality").tag(Engine.Mode.quality)
                Text("Strip").tag(Engine.Mode.strip)
            }
            .pickerStyle(.segmented)
            .frame(width: 220)
            .disabled(queue.isRunning)

            if queue.mode == .quality {
                Text("target \(Int(queue.target))")
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
                Slider(value: $queue.target, in: 60...90, step: 1)
                    .frame(width: 120)
                    .disabled(queue.isRunning)
            }

            Spacer()

            if !queue.jobs.isEmpty {
                Button("Clear", action: queue.clear)
                    .disabled(queue.isRunning)
            }
        }
        .padding(12)
    }

    private var dropPrompt: some View {
        VStack(spacing: 6) {
            Text("Drop images here")
                .font(.title3)
            Text(queue.mode == .strip ? "Removes metadata. Pixels are untouched." : "JPEG and PNG. Files are replaced in place.")
                .font(.callout)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var list: some View {
        List(queue.jobs) { job in
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(job.name)
                    Text(job.detail)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                if job.state == .working {
                    ProgressView().controlSize(.small)
                }
            }
            .padding(.vertical, 2)
        }
        .listStyle(.inset)
    }

    private func load(_ providers: [NSItemProvider]) {
        for provider in providers {
            _ = provider.loadObject(ofClass: URL.self) { url, _ in
                guard let url else { return }
                Task { @MainActor in queue.add([url], mode: queue.mode) }
            }
        }
    }
}
