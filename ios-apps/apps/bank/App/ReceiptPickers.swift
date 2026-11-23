import SwiftUI
import UIKit

/// Whether there is a camera to take a receipt with (the simulator has none).
var cameraIsAvailable: Bool {
    UIImagePickerController.isSourceTypeAvailable(.camera)
}

/// A JPEG of an image in a format Qonto does not accept (the photo library gives HEIC).
/// What `prepareReceipt` calls when the picked data is not a JPEG, a PNG or a PDF already.
func jpegData(from data: Data) -> Data? {
    UIImage(data: data)?.jpegData(compressionQuality: 0.85)
}

/// The system camera, for one photo. Calls back with the photo, or `nil` when it was cancelled:
/// the caller dismisses it.
struct CameraPicker: UIViewControllerRepresentable {
    let done: (UIImage?) -> Void

    init(_ done: @escaping (UIImage?) -> Void) {
        self.done = done
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(done)
    }

    func makeUIViewController(context: Context) -> UIImagePickerController {
        let picker = UIImagePickerController()
        picker.sourceType = .camera
        picker.delegate = context.coordinator
        return picker
    }

    func updateUIViewController(_ picker: UIImagePickerController, context: Context) {}

    final class Coordinator: NSObject, UIImagePickerControllerDelegate, UINavigationControllerDelegate {
        private let done: (UIImage?) -> Void

        init(_ done: @escaping (UIImage?) -> Void) {
            self.done = done
        }

        func imagePickerController(
            _ picker: UIImagePickerController,
            didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]
        ) {
            done(info[.originalImage] as? UIImage)
        }

        func imagePickerControllerDidCancel(_ picker: UIImagePickerController) {
            done(nil)
        }
    }
}
