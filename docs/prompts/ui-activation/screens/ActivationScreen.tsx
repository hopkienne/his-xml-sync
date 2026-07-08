import { useEffect, useRef, useState } from 'react';
import { Loader2, Shield } from 'lucide-react';
import { LicenseKeyInput } from '../components/LicenseKeyInput';
import { StatusMessage } from '../components/StatusMessage';
import { activateLicense, getLicenseStatus } from '../lib/licenseCommands';
import type { LicenseErrorCode, LicenseInfo } from '../types/license';
import './ActivationScreen.css';

type ScreenPhase = 'checking' | 'idle' | 'submitting' | 'success' | 'error';

interface ActivationScreenProps {
  /** Gọi khi license hợp lệ (đã có sẵn hoặc vừa kích hoạt) để chuyển vào màn hình chính. */
  onActivated: (info: LicenseInfo) => void;
}

/** Thời gian giữ màn hình tóm tắt trước khi tự động chuyển tiếp. */
const SUCCESS_HANDOFF_DELAY_MS = 1400;

export function ActivationScreen({ onActivated }: ActivationScreenProps) {
  const [phase, setPhase] = useState<ScreenPhase>('checking');
  const [key, setKey] = useState('');
  const [errorCode, setErrorCode] = useState<LicenseErrorCode | null>(null);
  const [activatedInfo, setActivatedInfo] = useState<LicenseInfo | null>(null);
  const handoffTimer = useRef<number | null>(null);

  // Kiểm tra license hiện có ngay khi màn hình được mount.
  useEffect(() => {
    let cancelled = false;

    getLicenseStatus().then((status) => {
      if (cancelled) return;
      if (status.valid && status.info) {
        onActivated(status.info);
        return;
      }
      setPhase('idle');
    });

    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    return () => {
      if (handoffTimer.current !== null) {
        window.clearTimeout(handoffTimer.current);
      }
    };
  }, []);

  const trimmedKey = key.trim();
  const canSubmit = phase === 'idle' || phase === 'error';
  const isBusy = phase === 'checking' || phase === 'submitting';

  async function handleSubmit(event: React.FormEvent) {
    event.preventDefault();
    if (!trimmedKey || !canSubmit) return;

    setPhase('submitting');
    setErrorCode(null);

    try {
      const info = await activateLicense(trimmedKey);
      setActivatedInfo(info);
      setPhase('success');
      handoffTimer.current = window.setTimeout(() => {
        onActivated(info);
      }, SUCCESS_HANDOFF_DELAY_MS);
    } catch (err) {
      const normalized = err as { code?: LicenseErrorCode };
      setErrorCode(normalized.code ?? 'UNKNOWN');
      setPhase('error');
    }
  }

  function handleRetry() {
    setKey('');
    setErrorCode(null);
    setPhase('idle');
  }

  const statusDotClass =
    phase === 'success'
      ? 'is-success'
      : phase === 'error'
        ? 'is-error'
        : isBusy
          ? 'is-busy'
          : 'is-idle';

  return (
    <div className="activation-screen">
      <div className="activation-panel">
        <header className="activation-panel__header">
          <div className="activation-panel__brand">
            <Shield size={20} strokeWidth={2} aria-hidden="true" />
            <span>HIS XML Sync</span>
          </div>
          <span className={`activation-status-dot ${statusDotClass}`} aria-hidden="true" />
        </header>

        <p className="activation-panel__subtitle">
          Nhập key kích hoạt để bắt đầu sử dụng ứng dụng.
        </p>

        {phase === 'checking' && (
          <div className="activation-checking">
            <Loader2 size={16} strokeWidth={2} className="spin" aria-hidden="true" />
            <span>Đang kiểm tra license hiện tại...</span>
          </div>
        )}

        {phase === 'success' && activatedInfo && (
          <StatusMessage variant="success" info={activatedInfo} />
        )}

        {(phase === 'idle' || phase === 'submitting' || phase === 'error') && (
          <form className="activation-form" onSubmit={handleSubmit}>
            <LicenseKeyInput value={key} onChange={setKey} disabled={phase === 'submitting'} />

            {phase === 'error' && errorCode && (
              <StatusMessage variant="error" code={errorCode} />
            )}

            <div className="activation-actions">
              <button
                type="submit"
                className="activation-button activation-button--primary"
                disabled={!trimmedKey || phase === 'submitting'}
              >
                {phase === 'submitting' ? (
                  <>
                    <Loader2 size={16} strokeWidth={2} className="spin" aria-hidden="true" />
                    <span>Đang xác thực...</span>
                  </>
                ) : (
                  <span>Kích hoạt</span>
                )}
              </button>

              {phase === 'error' && (
                <button
                  type="button"
                  className="activation-button activation-button--ghost"
                  onClick={handleRetry}
                >
                  Nhập key khác
                </button>
              )}
            </div>
          </form>
        )}
      </div>
    </div>
  );
}
