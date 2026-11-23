import FeedCore
import Foundation

/// Runs blocking work (fetching a feed in the Rust core is) off the calling actor.
func offMain<T: Sendable>(_ work: @escaping @Sendable () throws -> T) async throws -> T {
    try await withCheckedThrowingContinuation { continuation in
        DispatchQueue.global(qos: .userInitiated).async {
            continuation.resume(with: Result { try work() })
        }
    }
}

/// What to show for an error: the core's own sentence for its errors, the system's for the others.
public func message(for error: Error) -> String {
    if let error = error as? FeedError {
        return errorMessage(error: error)
    }
    return error.localizedDescription
}

/// What the app itself refuses.
public enum FeedKitError: LocalizedError, Equatable {
    case alreadyFollowed(String)

    public var errorDescription: String? {
        switch self {
        case .alreadyFollowed(let name): return "this feed is already in the list, as \(name)"
        }
    }
}

/// Fetches and reads a feed. The cookies only ever go to the feed's own server: see the core's `fetch_feed`.
func fetch(url: String, cookies: String?) async throws -> ParsedFeed {
    try await offMain { try fetchFeed(url: url, cookies: cookies) }
}

/// The name of a feed when the user gave none: the feed's own title, or else its host.
func defaultName(title: String, url: String) -> String {
    if !title.isEmpty {
        return title
    }
    return URL(string: url)?.host ?? url
}
