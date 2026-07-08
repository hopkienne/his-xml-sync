import { KeyRound } from 'lucide-react';

interface LicenseKeyInputProps {
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  id?: string;
}

export function LicenseKeyInput({
  value,
  onChange,
  disabled = false,
  id = 'license-key',
}: LicenseKeyInputProps) {
  return (
    <div className="license-field">
      <label className="license-field__label" htmlFor={id}>
        <KeyRound size={14} strokeWidth={2} aria-hidden="true" />
        <span>Key kích hoạt</span>
      </label>
      <textarea
        id={id}
        className="license-field__textarea"
        placeholder="Dán key kích hoạt được cung cấp vào đây..."
        value={value}
        disabled={disabled}
        spellCheck={false}
        autoComplete="off"
        autoCorrect="off"
        rows={5}
        onChange={(event) => onChange(event.target.value)}
      />
      <p className="license-field__hint">
        Key có thể dài nhiều dòng. Dán nguyên vẹn nội dung được cấp, không chỉnh sửa.
      </p>
    </div>
  );
}
