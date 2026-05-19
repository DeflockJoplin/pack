import { useCallback, useState } from 'react'
import { apiDelete, apiGet, apiPost } from '../api'
import { useApiLoad } from '../useApiLoad'

type FileEntry = {
  name: string
  size_bytes: number
  modified_ms: number | null
}

type WigleFiles = {
  pending: FileEntry[]
  uploaded: FileEntry[]
}

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`
  return `${(n / (1024 * 1024)).toFixed(2)} MiB`
}

function fmtTime(ms: number | null): string {
  if (ms == null) return '—'
  const d = new Date(ms)
  return Number.isNaN(d.getTime()) ? '—' : d.toLocaleString()
}

function downloadHref(bucket: 'pending' | 'uploaded', name: string): string {
  const enc = encodeURIComponent(name)
  return `/api/files/wigle/${bucket}/${enc}`
}

function FileTable({
  title,
  bucket,
  rows,
  onRefresh,
}: {
  title: string
  bucket: 'pending' | 'uploaded'
  rows: FileEntry[]
  onRefresh: () => void
}) {
  const [deleting, setDeleting] = useState<string | null>(null)

  const handleDelete = async (name: string) => {
    if (!window.confirm(`Delete session file ${name}? This cannot be undone.`)) return
    setDeleting(name)
    try {
      await apiDelete(`/api/files/wigle/${bucket}/${encodeURIComponent(name)}`)
      onRefresh()
    } catch (e) {
      window.alert(e instanceof Error ? e.message : String(e))
    } finally {
      setDeleting(null)
    }
  }

  return (
    <div className="panel">
      <h2>{title}</h2>
      {rows.length === 0 ? (
        <p className="muted">No files.</p>
      ) : (
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>Size</th>
              <th>Modified</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {rows.map((f) => (
              <tr key={f.name}>
                <td>
                  <code>{f.name}</code>
                </td>
                <td>{fmtBytes(f.size_bytes)}</td>
                <td className="muted">{fmtTime(f.modified_ms)}</td>
                <td>
                  <div className="row" style={{ gap: '0.5rem', flexWrap: 'wrap' }}>
                    <a className="button" href={downloadHref(bucket, f.name)} download={f.name}>
                      Download
                    </a>
                    <button
                      type="button"
                      className="secondary"
                      disabled={deleting === f.name}
                      onClick={() => void handleDelete(f.name)}
                    >
                      {deleting === f.name ? 'Deleting…' : 'Delete'}
                    </button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  )
}

const DB_RESET_CONFIRM =
  'Reset all local SQLite databases? This deletes wardrive.sqlite, cotravel.sqlite, and any other *.sqlite files in the data directory. WiGLE/recon CSV and PCAP files are NOT deleted. This cannot be undone. In-memory GPS track points may remain until restart.'

export function FilesPage() {
  const [data, setData] = useState<WigleFiles | null>(null)
  const [wigleLoading, setWigleLoading] = useState(true)
  const [err, setErr] = useState<string | null>(null)
  const [dbResetting, setDbResetting] = useState(false)
  const [dbResetMsg, setDbResetMsg] = useState<string | null>(null)
  const [dbResetErr, setDbResetErr] = useState<string | null>(null)

  const load = useCallback(async () => {
    setErr(null)
    setWigleLoading(true)
    try {
      setData(await apiGet<WigleFiles>('/api/files/wigle'))
    } catch (e) {
      setData(null)
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setWigleLoading(false)
    }
  }, [])

  useApiLoad(() => void load(), [load])

  const resetDatabases = async () => {
    if (!window.confirm(DB_RESET_CONFIRM)) return
    setDbResetMsg(null)
    setDbResetErr(null)
    setDbResetting(true)
    try {
      const r = await apiPost<{ ok: boolean; removed: string[] }>('/api/files/databases/reset', {})
      const names = r.removed.length ? r.removed.join(', ') : '(none)'
      setDbResetMsg(`Local databases reset. Removed: ${names}.`)
    } catch (e) {
      setDbResetErr(e instanceof Error ? e.message : String(e))
    } finally {
      setDbResetting(false)
    }
  }

  return (
    <div className="stack">
      <div className="page-head">
        <h2 className="page-head__title">Files</h2>
        <p className="muted page-head__lead">
          WiGLE CSV sessions under <code>data/wigle/pending</code> and <code>data/wigle/uploaded</code> (see{' '}
          <strong>Uploads</strong> to run the uploader).
        </p>
        <div className="page-head__row">
          <button type="button" className="secondary" onClick={() => void load()}>
            Refresh
          </button>
        </div>
      </div>
      {dbResetErr && <div className="panel panel--error">{dbResetErr}</div>}
      {dbResetMsg && (
        <div className="panel" role="status">
          <p className="msg-ok" style={{ margin: 0 }}>
            {dbResetMsg}
          </p>
        </div>
      )}
      <div className="panel">
        <h2>Local databases</h2>
        <p className="muted">
          SQLite files under the data directory (<code>wardrive.sqlite</code>, <code>cotravel.sqlite</code>, etc.). CSV
          and PCAP exports are not affected.
        </p>
        <button
          type="button"
          className="secondary"
          disabled={dbResetting}
          onClick={() => void resetDatabases()}
        >
          {dbResetting ? 'Resetting…' : 'Reset local databases'}
        </button>
      </div>
      {wigleLoading && (
        <div className="panel">
          <p className="muted" style={{ margin: 0 }}>
            Loading WiGLE file lists…
          </p>
        </div>
      )}
      {err && !wigleLoading && <div className="panel panel--error">{err}</div>}
      {data && !wigleLoading && (
        <>
          <FileTable title="Pending" bucket="pending" rows={data.pending} onRefresh={() => void load()} />
          <FileTable title="Uploaded" bucket="uploaded" rows={data.uploaded} onRefresh={() => void load()} />
        </>
      )}
    </div>
  )
}
