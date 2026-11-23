import AVFoundation
import SwiftUI

/// The camera, looking for a QR code. Gives its text once; what the text means is not decided here.
struct ScannerSheet: View {
    let found: (String) -> Void

    @Environment(\.dismiss) private var dismiss
    /// `nil` until the user has answered (or had already answered) the camera question.
    @State private var allowed: Bool?

    var body: some View {
        NavigationStack {
            Group {
                switch allowed {
                case .some(true):
                    ScannerView(found: found)
                        .ignoresSafeArea(edges: .bottom)
                case .some(false):
                    ContentUnavailableView(
                        "No camera",
                        systemImage: "video.slash",
                        description: Text("Allow the camera for 2fa in Settings, or enter the key by hand."))
                case .none:
                    ProgressView()
                }
            }
            .navigationTitle("Scan a QR code")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
        }
        .task {
            allowed = await AVCaptureDevice.requestAccess(for: .video)
        }
    }
}

struct ScannerView: UIViewControllerRepresentable {
    let found: (String) -> Void

    func makeUIViewController(context: Context) -> ScannerController {
        ScannerController(found: found)
    }

    func updateUIViewController(_ controller: ScannerController, context: Context) {}
}

final class ScannerController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    private let session = AVCaptureSession()
    private let found: (String) -> Void
    private var preview: AVCaptureVideoPreviewLayer?
    private var done = false

    init(found: @escaping (String) -> Void) {
        self.found = found
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("not in a storyboard")
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        let output = AVCaptureMetadataOutput()
        guard let camera = AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: camera),
              session.canAddInput(input), session.canAddOutput(output)
        else { return } // the simulator: a black sheet, and Cancel
        session.addInput(input)
        session.addOutput(output)
        // After it is added: before, it knows no type.
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]

        let preview = AVCaptureVideoPreviewLayer(session: session)
        preview.videoGravity = .resizeAspectFill
        view.layer.addSublayer(preview)
        self.preview = preview
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        preview?.frame = view.bounds
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        // Blocks until the camera runs: not on the main thread.
        DispatchQueue.global(qos: .userInitiated).async { [session] in
            session.startRunning()
        }
    }

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        DispatchQueue.global(qos: .userInitiated).async { [session] in
            session.stopRunning()
        }
    }

    func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput objects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        guard !done, let code = objects.first as? AVMetadataMachineReadableCodeObject, let text = code.stringValue else {
            return
        }
        done = true
        found(text)
    }
}
