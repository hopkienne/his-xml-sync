import { useState } from "react";
import "./App.css";
import { ActivationScreen } from "./screens/ActivationScreen";
import { HomeShell } from "./screens/HomeShell";
import type { LicenseInfo } from "./types/license";

export type AppSession = {
  isActivated: boolean;
  customerName?: string;
  facilityName?: string;
  expiresAt?: string;
};

const initialSession: AppSession = {
  isActivated: false,
};

function App() {
  const [session, setSession] = useState<AppSession>(initialSession);

  function handleActivated(info: LicenseInfo) {
    setSession({
      isActivated: true,
      customerName: info.customerName,
      facilityName: info.facilityName,
      expiresAt: info.expiresAt,
    });
  }

  if (!session.isActivated) {
    return <ActivationScreen onActivated={handleActivated} />;
  }

  return <HomeShell session={session} onLogout={() => setSession(initialSession)} />;
}

export default App;
