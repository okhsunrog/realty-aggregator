const API_BASE = ''

export class ApiError extends Error {
  constructor(public status: number, public statusText: string, public body?: any) {
    super(typeof body?.error === 'string' ? body.error : `${status} ${statusText}`)
    this.name = 'ApiError'
  }
}

async function handleResponse<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const body = await res.json().catch(() => null)
    throw new ApiError(res.status, res.statusText, body)
  }
  return res.json()
}

export interface Listing {
  id: number
  source: string
  url: string
  title: string | null
  description: string | null
  price: number | null
  city: string
  address: string | null
  lat: number | null
  lng: number | null
  rooms: number | null
  area: number | null
  floor: number | null
  floors_total: number | null
  is_active: boolean
  fetched_at: string
  analysis_analyzed_at: string | null
  analysis_attempts: number
  analysis_last_error: string | null
  actuality_checked_at: string | null
  actuality_origin: string
  source_time: string | null
  source_check_status: 'active' | 'inactive' | 'unknown' | null
  source_checked_at: string | null
  source_check_http_status: number | null
  source_check_reason: string | null
  // Analysis fields
  analysis_condition: number | null
  analysis_is_shell: boolean | null
  analysis_move_in_ready: boolean | null
  analysis_red_flags: string[] | null
  analysis_summary: string | null
  analysis_model: string | null
}

export interface ListingImage {
  id: number
  listing_id: number
  image_index: number
  source_url: string
  s3_key: string | null
  status: string
  ai_image_type: string | null
  ai_room: string | null
  ai_analyzed_at: string | null
  ai_last_error: string | null
  ai_condition: number | null
  ai_confidence: number | null
  ai_issues: string[] | null
  ai_description: string | null
}

export interface Stats {
  total_listings: number
  active_listings: number
  images: {
    total: number
    downloaded: number
    pending: number
    failed: number
  }
  analysis: {
    images_analyzed: number
    images_pending: number
    listings_analyzed: number
    listings_pending: number
  }
}

export interface ListingsResponse {
  listings: Listing[]
  total: number
  page: number
  per_page: number
}

export async function fetchStats(): Promise<Stats> {
  const res = await fetch(`${API_BASE}/api/stats`)
  return handleResponse<Stats>(res)
}

export async function fetchListings(params: {
  verified_only?: boolean
  min_area?: number
  move_in_ready?: boolean
  favorites_only?: boolean
  hide_rejected?: boolean
  page?: number
  per_page?: number
  min_price?: number
  max_price?: number
  rooms?: number
  analyzed_only?: boolean
  hide_shell?: boolean
  active_only?: boolean
  property_type?: string
  is_shell?: boolean
  min_condition?: number
  max_condition?: number
}, signal?: AbortSignal): Promise<ListingsResponse> {
  const query = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== null && value !== '') query.set(key, String(value))
  }
  const res = await fetch(`${API_BASE}/api/listings?${query}`, { signal })
  return handleResponse<ListingsResponse>(res)
}

export async function fetchListing(id: number): Promise<Listing> {
  const res = await fetch(`${API_BASE}/api/listings/${id}`)
  return handleResponse<Listing>(res)
}

export async function fetchListingImages(listingId: number): Promise<ListingImage[]> {
  const res = await fetch(`${API_BASE}/api/listings/${listingId}/images`)
  return handleResponse<ListingImage[]>(res)
}

export function getImageUrl(imageId: number): string {
  return `${API_BASE}/api/images/${imageId}/content`
}

export async function markListingInactive(id: number): Promise<{ success: boolean; id: number; is_active: boolean }> {
  const res = await fetch(`${API_BASE}/api/listings/${id}/mark-inactive`, {
    method: 'POST'
  })
  return handleResponse<{ success: boolean; id: number; is_active: boolean }>(res)
}

export interface CollectionResponse {
  message?: string
  error?: string
}

export async function triggerCollection(): Promise<CollectionResponse> {
  const res = await fetch(`${API_BASE}/api/collect`, {
    method: 'POST'
  })
  // 409 = already running, не ошибка
  if (res.status === 409) {
    return { error: 'already_running' }
  }
  return handleResponse<CollectionResponse>(res)
}

export async function getCollectionStatus(): Promise<{ is_running: boolean }> {
  const res = await fetch(`${API_BASE}/api/collect/status`)
  return handleResponse<{ is_running: boolean }>(res)
}

export type JobPhase = 'idle' | 'collecting_listings' | 'downloading_images' | 'analyzing_images' | 'analyzing_listings' | 'syncing_actuality'

export interface JobProgress {
  processed: number
  total: number | null
}

export interface JobStatus {
  phase: JobPhase
  progress: JobProgress | null
  started_at: string | null
  message: string | null
  last_error: string | null
}

export async function getJobStatus(): Promise<JobStatus> {
  const res = await fetch(`${API_BASE}/api/status`)
  return handleResponse<JobStatus>(res)
}

export async function startAnalysis(id: number, images: boolean, model?: string, signal?: AbortSignal): Promise<CollectionResponse> {
  const res = await fetch(`/api/listings/${id}/analyze${images ? '/images' : ''}`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(model ? { model } : null), signal,
  })
  return handleResponse<CollectionResponse>(res)
}
export interface ListingReview { favorite: boolean; hidden: boolean; stage: string; notes: string }
export interface PriceObservation { observed_at: string; price: number }
export async function fetchReview(id: number): Promise<ListingReview> {
  return handleResponse(await fetch(`/api/listings/${id}/review`))
}
export async function saveReview(id: number, review: ListingReview): Promise<ListingReview> {
  return handleResponse(await fetch(`/api/listings/${id}/review`, {
    method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(review),
  }))
}
export async function fetchPriceHistory(id: number): Promise<PriceObservation[]> {
  return handleResponse(await fetch(`/api/listings/${id}/price-history`))
}

export async function startSourceCheck(id: number, signal?: AbortSignal): Promise<CollectionResponse> {
  return handleResponse(await fetch(`/api/listings/${id}/check-source`, { method: 'POST', signal }))
}
export function sourceStatusLabel(listing: Listing): string {
  if (!listing.is_active) return 'Неактивно'
  if (listing.source_time && listing.source_checked_at && Date.parse(listing.source_time) > Date.parse(listing.source_checked_at)) return 'Нужна повторная проверка'
  if (listing.source_check_status === 'active') return 'Проверено: активно'
  if (listing.source_check_status === 'inactive') return 'Проверено: недоступно'
  return 'Актуальность не подтверждена'
}

export function sourceOriginLabel(origin: string): string {
  return ({ source_page: 'исходная карточка', check_feed: 'проверка ADS API', source_feed: 'лента объявлений', manual: 'ручная отметка' } as Record<string, string>)[origin] ?? 'источник'
}
export function imageTypeLabel(type: string): string {
  return ({ interior: 'Интерьер', exterior: 'Дом или двор', floor_plan: 'Планировка', other: 'Другое' } as Record<string, string>)[type] ?? type
}
export function roomLabel(room: string): string {
  return ({ bathroom: 'Санузел', kitchen: 'Кухня', living: 'Жилая комната', bedroom: 'Спальня', hallway: 'Прихожая' } as Record<string, string>)[room] ?? room
}

export async function fetchSettings(): Promise<{ models: string[]; default_model: string; source_check_ttl_hours: number }> {
  return handleResponse(await fetch('/api/settings'))
}
