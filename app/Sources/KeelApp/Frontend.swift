import Foundation

/// What counts as "the frontend", and which page a file is.
///
/// Both are heuristics and both are allowed to say "don't know": a wrong answer navigates the
/// preview to the wrong page in the middle of someone watching, which is worse than staying put.
/// A component edit shows up wherever the component is rendered, and the page's own mutation
/// observer sees that without any mapping at all.
enum Frontend {
    private static let uiExtensions: Set<String> = [
        "tsx", "jsx", "ts", "js", "vue", "svelte", "astro", "css", "scss", "html", "mdx",
    ]
    private static let notUI: Set<String> = [
        "backend", "server", "api", "test", "tests", "__tests__", "node_modules", "infra",
        "scripts", "worker", "workers",
    ]

    /// Whether editing this file can change what the preview shows.
    static func isUI(_ path: String) -> Bool {
        let parts = path.split(separator: "/").map(String.init)
        guard let file = parts.last else { return false }
        let ext = (file as NSString).pathExtension.lowercased()
        guard uiExtensions.contains(ext) else { return false }
        let name = (file as NSString).deletingPathExtension
        if name.hasSuffix(".test") || name.hasSuffix(".spec") || name.hasSuffix(".config") {
            return false
        }
        return !parts.dropLast().contains { notUI.contains($0.lowercased()) }
    }

    /// The URL path a page file renders, when that is unambiguous.
    ///
    /// Next.js App Router (`app/**/page.tsx`, groups dropped), Pages Router (`pages/**`), and
    /// SvelteKit (`routes/**/+page.svelte`). A dynamic segment — `[id]`, `[...slug]` — is `nil`:
    /// there is no one page to show for it.
    static func route(for path: String) -> String? {
        let parts = path.split(separator: "/").map(String.init)
        guard let file = parts.last else { return nil }
        let base = (file as NSString).deletingPathExtension

        func clean(_ segments: ArraySlice<String>) -> String? {
            var out: [String] = []
            for s in segments {
                if s.hasPrefix("(") && s.hasSuffix(")") { continue }   // route group
                if s.hasPrefix("[") || s.hasPrefix("@") { return nil }  // dynamic / parallel
                if s.hasPrefix("_") { return nil }                       // private folder
                out.append(s)
            }
            return "/" + out.joined(separator: "/")
        }

        // App Router: the last `app` directory before the file.
        if base == "page", let i = parts.lastIndex(of: "app"), i < parts.count - 1 {
            return clean(parts[(i + 1)..<(parts.count - 1)])
        }
        // SvelteKit.
        if base == "+page", let i = parts.lastIndex(of: "routes") {
            return clean(parts[(i + 1)..<(parts.count - 1)])
        }
        // Pages Router: `pages/a/b.tsx` → `/a/b`, `index` drops.
        if let i = parts.lastIndex(of: "pages"), i < parts.count - 1 {
            if base.hasPrefix("_") || parts[i + 1] == "api" { return nil }
            var segs = Array(parts[(i + 1)..<(parts.count - 1)])
            if base != "index" { segs.append(base) }
            return clean(segs[...])
        }
        return nil
    }

    /// The page a written file *appears on*, when the file is not itself a page.
    ///
    /// `route(for:)` answers for `app/**/page.tsx` and its siblings and stays quiet otherwise,
    /// which is right — but "stays quiet" meant the preview sat on whatever it was already showing
    /// while the bar said "Editing domain-step.tsx…". A component is not a page; it is *on* one,
    /// and which one is a walk up the import graph.
    ///
    /// Breadth-first from the edited file, asking the daemon who imports each level. It stops at
    /// the first level that yields a route, and only navigates when that level yields exactly one
    /// — two candidates means the component is shared, and picking one of them would move the
    /// preview off the page somebody is watching for no better reason than alphabetical order.
    ///
    /// Three levels, because a component nested deeper than that is shared by construction and the
    /// answer would be ambiguous anyway.
    static func page(containing path: String, client: Client, query: [String: String]) async -> String? {
        if let direct = route(for: path) { return direct }

        var seen: Set<String> = [path]
        var level = [path]
        for _ in 0..<3 {
            var next: [String] = []
            for file in level {
                var q = query
                q["file"] = file
                let found: [String] = (try? await client.get("/api/frontend/importers", q)) ?? []
                next.append(contentsOf: found.filter { seen.insert($0).inserted })
            }
            guard !next.isEmpty else { return nil }

            let routes = Set(next.compactMap { route(for: $0) })
            if routes.count == 1 { return routes.first }
            // Several pages render it, so there is no one page to show.
            if routes.count > 1 { return nil }
            level = next
        }
        return nil
    }

    /// `http://127.0.0.1:3000/anything` → `http://127.0.0.1:3000`.
    static func origin(of url: String) -> String? {
        guard let u = URL(string: url), let scheme = u.scheme, let host = u.host else { return nil }
        return u.port.map { "\(scheme)://\(host):\($0)" } ?? "\(scheme)://\(host)"
    }
}
