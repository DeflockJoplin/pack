import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom'
import { Layout } from './Layout'
import { Adapters } from './pages/Adapters'
import { ChannelPlanPage } from './pages/ChannelPlan'
import { Dashboard } from './pages/Dashboard'
import { Diagnostics } from './pages/Diagnostics'
import { FilesPage } from './pages/Files'
import { ReconPage } from './pages/Recon'
import { UploadsPage } from './pages/Uploads'
import { CoTravelPage } from './pages/CoTravel'
import { MapPage } from './pages/Map'
import { Nearby } from './pages/Nearby'
import { DetectionsPage } from './pages/Detections'
import { PrivacyPage } from './pages/Privacy'

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route element={<Layout />}>
          <Route index element={<Dashboard />} />
          <Route path="adapters" element={<Adapters />} />
          <Route path="monitor" element={<Navigate to="/adapters" replace />} />
          <Route path="channels" element={<ChannelPlanPage />} />
          <Route path="files" element={<FilesPage />} />
          <Route path="uploads" element={<UploadsPage />} />
          <Route path="recon" element={<ReconPage />} />
          <Route path="map" element={<MapPage />} />
          <Route path="nearby" element={<Nearby />} />
          <Route path="cotravel" element={<CoTravelPage />} />
          <Route path="detections" element={<DetectionsPage />} />
          <Route path="flock-wifi" element={<Navigate to="/detections" replace />} />
          <Route path="diagnostics" element={<Diagnostics />} />
          <Route path="privacy" element={<PrivacyPage />} />
          <Route path="home-zone" element={<Navigate to="/privacy" replace />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Route>
      </Routes>
    </BrowserRouter>
  )
}
