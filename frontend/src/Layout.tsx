import { useState } from 'react'
import { NavLink, Outlet } from 'react-router-dom'
import { apiPost } from './api'
import { BootUploadBanner } from './BootUploadBanner'

export function Layout() {
  const [quitMsg, setQuitMsg] = useState<string | null>(null)
  const [quitErr, setQuitErr] = useState<string | null>(null)

  const quitDaemon = async () => {
    if (!window.confirm('Stop PACK?')) return
    setQuitMsg(null)
    setQuitErr(null)
    try {
      await apiPost<{ ok: boolean }>('/api/shutdown', {})
      setQuitMsg('Shutting down…')
    } catch (e) {
      setQuitErr(e instanceof Error ? e.message : String(e))
    }
  }
  return (
    <div className="layout">
      <header className="app-header">
        <div className="app-header__brand">
          <h1>P.A.C.K.</h1>
          <p className="app-tagline">Passive Acquisition &amp; Capture Kit</p>
        </div>
        <nav className="app-nav" aria-label="Main">
          <div className="nav-group">
            <span className="nav-group__label">Run</span>
            <div className="nav-group__links">
              <NavLink to="/" end className={({ isActive }) => (isActive ? 'active' : undefined)}>
                Dashboard
              </NavLink>
              <NavLink to="/nearby" className={({ isActive }) => (isActive ? 'active' : undefined)}>
                Nearby
              </NavLink>
              <NavLink to="/map" className={({ isActive }) => (isActive ? 'active' : undefined)}>
                Map
              </NavLink>
              <NavLink to="/cotravel" className={({ isActive }) => (isActive ? 'active' : undefined)}>
                Co-travel
              </NavLink>
              <NavLink to="/detections" className={({ isActive }) => (isActive ? 'active' : undefined)}>
                Detections
              </NavLink>
            </div>
          </div>
          <div className="nav-group">
            <span className="nav-group__label">Radios</span>
            <div className="nav-group__links">
              <NavLink to="/adapters" className={({ isActive }) => (isActive ? 'active' : undefined)}>
                Adapters
              </NavLink>
              <NavLink to="/channels" className={({ isActive }) => (isActive ? 'active' : undefined)}>
                Channel plan
              </NavLink>
            </div>
          </div>
          <div className="nav-group">
            <span className="nav-group__label">Data</span>
            <div className="nav-group__links">
              <NavLink to="/files" className={({ isActive }) => (isActive ? 'active' : undefined)}>
                Files
              </NavLink>
              <NavLink to="/uploads" className={({ isActive }) => (isActive ? 'active' : undefined)}>
                Uploads
              </NavLink>
              <NavLink to="/recon" className={({ isActive }) => (isActive ? 'active' : undefined)}>
                Recon
              </NavLink>
            </div>
          </div>
          <div className="nav-group">
            <span className="nav-group__label">System</span>
            <div className="nav-group__links">
              <NavLink to="/diagnostics" className={({ isActive }) => (isActive ? 'active' : undefined)}>
                Diagnostics
              </NavLink>
              <NavLink to="/privacy" className={({ isActive }) => (isActive ? 'active' : undefined)}>
                Privacy
              </NavLink>
              <button type="button" className="nav-quit" onClick={() => void quitDaemon()}>
                Quit daemon
              </button>
            </div>
            {(quitMsg || quitErr) && (
              <p className={`nav-quit-status${quitErr ? ' nav-quit-status--error' : ''}`} role="status">
                {quitErr ?? quitMsg}
              </p>
            )}
          </div>
        </nav>
      </header>
      <BootUploadBanner />
      <main>
        <Outlet />
      </main>
    </div>
  )
}
