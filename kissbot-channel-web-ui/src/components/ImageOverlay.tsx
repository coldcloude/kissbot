interface ImageOverlayProps {
  src: string;
  onClose: () => void;
}

export default function ImageOverlay({ src, onClose }: ImageOverlayProps) {
  return (
    <div className="image-overlay" onClick={onClose}>
      <img src={src} alt="" onClick={e => e.stopPropagation()} />
    </div>
  );
}
