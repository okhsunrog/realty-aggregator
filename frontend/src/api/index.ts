const API_BASE = 'http://localhost:3000'

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
  // Analysis fields
  analysis_condition: number | null
  analysis_is_shell: boolean | null
  analysis_move_in_ready: boolean | null
  analysis_red_flags: string[] | null
  analysis_summary: string | null
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
  return res.json()
}

export async function fetchListings(params: {
  page?: number
  per_page?: number
  min_price?: number
  max_price?: number
  rooms?: number
  analyzed_only?: boolean
  hide_shell?: boolean
  active_only?: boolean
}): Promise<ListingsResponse> {
  const query = new URLSearchParams()
  if (params.page) query.set('page', params.page.toString())
  if (params.per_page) query.set('per_page', params.per_page.toString())
  if (params.min_price) query.set('min_price', params.min_price.toString())
  if (params.max_price) query.set('max_price', params.max_price.toString())
  if (params.rooms) query.set('rooms', params.rooms.toString())
  if (params.analyzed_only) query.set('analyzed_only', 'true')
  if (params.hide_shell) query.set('hide_shell', 'true')
  if (params.active_only) query.set('active_only', 'true')

  const res = await fetch(`${API_BASE}/api/listings?${query}`)
  return res.json()
}

export async function fetchListing(id: number): Promise<Listing> {
  const res = await fetch(`${API_BASE}/api/listings/${id}`)
  return res.json()
}

export async function fetchListingImages(listingId: number): Promise<ListingImage[]> {
  const res = await fetch(`${API_BASE}/api/listings/${listingId}/images`)
  return res.json()
}

export function getImageUrl(s3Key: string): string {
  return `http://localhost:9000/realty-images/${s3Key}`
}
