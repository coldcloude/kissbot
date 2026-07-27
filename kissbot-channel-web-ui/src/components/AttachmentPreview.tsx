interface AttachmentPreviewProps {
  files: File[];
  onRemove: (index: number) => void;
}

export default function AttachmentPreview({ files, onRemove }: AttachmentPreviewProps) {
  if (files.length === 0) return null;
  return (
    <div className="attachment-preview">
      {files.map((file, i) => (
        <div key={i} className="attachment-preview-item">
          <span>📎 {file.name}</span>
          <button onClick={() => onRemove(i)}>×</button>
        </div>
      ))}
    </div>
  );
}
