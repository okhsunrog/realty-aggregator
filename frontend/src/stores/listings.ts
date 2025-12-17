import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { fetchListings, fetchStats, type Listing, type Stats } from '@/api'

export const useListingsStore = defineStore('listings', () => {
  const listings = ref<Listing[]>([])
  const stats = ref<Stats | null>(null)
  const loading = ref(false)
  const error = ref<string | null>(null)
  const currentPage = ref(1)
  const totalListings = ref(0)
  const perPage = ref(20)

  // Filters
  const filters = ref({
    minPrice: undefined as number | undefined,
    maxPrice: undefined as number | undefined,
    rooms: undefined as number | undefined,
    analyzedOnly: false,
    hideShell: false,
    activeOnly: true,
  })

  const totalPages = computed(() => Math.ceil(totalListings.value / perPage.value))

  async function loadListings(page = 1) {
    loading.value = true
    error.value = null
    try {
      const response = await fetchListings({
        page,
        per_page: perPage.value,
        min_price: filters.value.minPrice,
        max_price: filters.value.maxPrice,
        rooms: filters.value.rooms,
        analyzed_only: filters.value.analyzedOnly,
        hide_shell: filters.value.hideShell,
        active_only: filters.value.activeOnly,
      })
      listings.value = response.listings
      totalListings.value = response.total
      currentPage.value = response.page
    } catch (e) {
      error.value = e instanceof Error ? e.message : 'Failed to load listings'
    } finally {
      loading.value = false
    }
  }

  async function loadStats() {
    try {
      stats.value = await fetchStats()
    } catch (e) {
      console.error('Failed to load stats:', e)
    }
  }

  function setFilters(newFilters: Partial<typeof filters.value>) {
    filters.value = { ...filters.value, ...newFilters }
    loadListings(1)
  }

  return {
    listings,
    stats,
    loading,
    error,
    currentPage,
    totalListings,
    perPage,
    totalPages,
    filters,
    loadListings,
    loadStats,
    setFilters,
  }
})
