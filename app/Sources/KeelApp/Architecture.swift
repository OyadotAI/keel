import SwiftUI

/// The architecture drawn, not listed.
///
/// Components are sorted into tiers by what they are — edge, app, API, data, workers — and
/// drawn as boxes with the request path running through them. The prose is still there for
/// the agent (`Template.architecture`); a person gets the picture and hovers for the rest.
struct ArchitectureDiagram: View {
    let template: Template

    private enum Tier { case edge, app, api, data, worker, other }

    private static func tier(_ c: Template.Component) -> Tier {
        let t = (c.tech + " " + c.name + " " + c.role).lowercased()
        if t.contains("nginx") || t.contains("ingress") || t.contains("gateway") && !t.contains("api gateway") { return .edge }
        if t.contains("next") || t.contains(" ui") || t.contains("console") || t.contains("page") { return .app }
        if t.contains("postgres") || t.contains("redis") || t.contains("storage") || t.contains("table") || t.contains("markdown") { return .data }
        if t.contains("worker") || t.contains("cron") || t.contains("scheduler") || t.contains("executor") || t.contains("runtime") || t.contains("reconciler") || t.contains("runner") || t.contains("model call") { return .worker }
        if t.contains("hono") || t.contains("api") || t.contains("proxy") || t.contains("endpoint") || t.contains("middleware") || t.contains("sdk") { return .api }
        return .other
    }

    private static func icon(_ c: Template.Component) -> String {
        let t = c.tech.lowercased()
        if t.contains("postgres") { return "cylinder" }
        if t.contains("redis") { return "bolt" }
        if t.contains("next") { return "macwindow" }
        if t.contains("nginx") || t.contains("ingress") { return "arrow.triangle.branch" }
        if t.contains("worker") || t.contains("cron") { return "gearshape.2" }
        if t.contains("model") || t.contains("claude") { return "sparkles" }
        if t.contains("markdown") { return "doc.text" }
        if t.contains("container") { return "shippingbox" }
        if t.contains("storage") { return "externaldrive" }
        return "server.rack"
    }

    private func those(_ tier: Tier) -> [Template.Component] { template.components.filter { Self.tier($0) == tier } }

    var body: some View {
        let edge = those(.edge), app = those(.app), api = those(.api)
        let data = those(.data), workers = those(.worker), other = those(.other)
        VStack(spacing: K.S.sm) {
            // The request path: client through the tiers that face it.
            HStack(spacing: K.S.xs) {
                chip(name: "Client", tech: "browser / caller", icon: "person", tone: K.C.faint)
                ForEach([edge, app, api].filter { !$0.isEmpty }, id: \.first!.name) { tier in
                    arrow
                    VStack(spacing: K.S.xs) { ForEach(tier, id: \.name) { box($0) } }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            // What hangs off the API: data on one side, workers on the other.
            if !data.isEmpty || !workers.isEmpty || !other.isEmpty {
                HStack(alignment: .top, spacing: K.S.md) {
                    Rectangle().fill(K.C.line).frame(width: 1, height: 14).padding(.leading, 60)
                    Spacer(minLength: 0)
                }
                HStack(alignment: .top, spacing: K.S.md) {
                    if !data.isEmpty { group("DATA", data) }
                    if !workers.isEmpty { group("WORKERS", workers) }
                    if !other.isEmpty { group("ALSO", other) }
                    Spacer(minLength: 0)
                }
            }
        }
        .padding(K.S.md)
        .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.md))
        .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.line, lineWidth: 1))
    }

    private var arrow: some View {
        Image(systemName: "arrow.right").font(.system(size: 10, weight: .semibold)).foregroundStyle(K.C.faint)
    }

    private func group(_ title: String, _ items: [Template.Component]) -> some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            Text(title).font(.system(size: 10, weight: .semibold)).tracking(0.7).foregroundStyle(K.C.faint)
            Flow(spacing: K.S.xs) { ForEach(items, id: \.name) { box($0) } }
        }
    }

    private func box(_ c: Template.Component) -> some View {
        chip(name: c.name, tech: c.tech, icon: Self.icon(c), tone: K.C.accent)
            .help("\(c.name) — \(c.role) (\(c.tech))")
    }

    private func chip(name: String, tech: String, icon: String, tone: Color) -> some View {
        HStack(spacing: K.S.xs) {
            Image(systemName: icon).font(.system(size: 11)).foregroundStyle(tone)
            VStack(alignment: .leading, spacing: 0) {
                Text(name).font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text).lineLimit(1)
                Text(tech).font(K.F.mono(10)).foregroundStyle(K.C.faint).lineLimit(1)
            }
        }
        .padding(.horizontal, K.S.sm).padding(.vertical, 5)
        .background(K.C.raised, in: RoundedRectangle(cornerRadius: K.R.sm))
        .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
    }
}

/// The request path as numbered steps, three words each, the sentence on hover.
struct FlowStrip: View {
    let steps: [String]

    var body: some View {
        Flow(spacing: K.S.xs) {
            ForEach(Array(steps.enumerated()), id: \.offset) { i, s in
                HStack(spacing: 5) {
                    Text("\(i + 1)").font(.system(size: 10, weight: .bold)).foregroundStyle(.white)
                        .frame(width: 16, height: 16).background(K.C.accent, in: Circle())
                    Text(Self.short(s)).font(K.F.small).foregroundStyle(K.C.dim).lineLimit(1)
                    if i < steps.count - 1 {
                        Image(systemName: "chevron.right").font(.system(size: 10, weight: .bold)).foregroundStyle(K.C.faint)
                    }
                }
                .help(s)
            }
        }
    }

    static func short(_ s: String) -> String {
        let cut = s.split(whereSeparator: { $0 == ";" || $0 == ":" || $0 == "," }).first.map(String.init) ?? s
        let words = cut.split(separator: " ").prefix(4).joined(separator: " ")
        return words.count < cut.count ? words + "…" : words
    }
}

/// What is built in, as pills: a few words each, the rule on hover.
struct PracticePills: View {
    let practices: [String]

    var body: some View {
        Flow(spacing: K.S.xs) {
            ForEach(practices, id: \.self) { p in
                HStack(spacing: 4) {
                    Image(systemName: Self.icon(p)).font(.system(size: 10)).foregroundStyle(K.C.add)
                    Text(Self.short(p)).font(K.F.small).foregroundStyle(K.C.text).lineLimit(1)
                }
                .padding(.horizontal, K.S.sm).padding(.vertical, 4)
                .background(K.C.add.opacity(0.08), in: Capsule())
                .overlay(Capsule().stroke(K.C.add.opacity(0.25), lineWidth: 1))
                .help(p)
            }
        }
    }

    static func short(_ p: String) -> String {
        let head = p.split(whereSeparator: { $0 == ":" || $0 == "—" || $0 == ";" }).first.map(String.init) ?? p
        let t = head.trimmingCharacters(in: .whitespaces)
        return t.count > 34 ? String(t.prefix(33)) + "…" : t
    }

    static func icon(_ p: String) -> String {
        let l = p.lowercased()
        if l.contains("secret") || l.contains("token") || l.contains("key") { return "lock" }
        if l.contains("rollout") || l.contains("probe") || l.contains("drain") { return "arrow.triangle.2.circlepath" }
        if l.contains("idempot") || l.contains("retry") || l.contains("replay") { return "repeat" }
        if l.contains("typed") || l.contains("test") { return "checkmark.seal" }
        if l.contains("log") || l.contains("sentry") || l.contains("observ") { return "waveform.path.ecg" }
        if l.contains("budget") || l.contains("limit") { return "gauge" }
        return "checkmark.shield"
    }
}
