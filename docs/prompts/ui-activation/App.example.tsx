/**
 * Ví dụ cách gắn ActivationScreen vào App gốc.
 * Không phải file bắt buộc — tham khảo để nối vào router/App thật của bạn.
 */
import { useState } from 'react';
import { ActivationScreen } from './screens/ActivationScreen';
import type { LicenseInfo } from './types/license';
// import { MainApp } from './screens/MainApp';

export default function App() {
  const [license, setLicense] = useState<LicenseInfo | null>(null);

  if (!license) {
    return <ActivationScreen onActivated={setLicense} />;
  }

  // Khi license hết hạn ở giữa phiên làm việc, đặt lại setLicense(null)
  // từ nơi phát hiện lỗi (ví dụ interceptor gọi API HIS) để tự động quay
  // lại màn hình này.
  return <div>{/* <MainApp license={license} /> */}Màn hình chính</div>;
}
