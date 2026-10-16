import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { fetchListings, fetchStats, ApiError, type Listing, type Stats } from '@/api'

export const useListingsStore = defineStore('listings', () => {
  const listings = ref<Listing[]>([])
  const stats = ref<Stats | null>(null)
  const statsLoading = ref(false)
  const loading = ref(false)
  const error = ref<string | null>(null)
  const currentPage = ref(1)
  const totalListings = ref(0)
  const perPage = ref(20)

  let requestId = 0
  let controller: AbortController | undefined

  // Filters
  const filters = ref({
    minArea: undefined as number | undefined,
    moveInReady: false,
    favoritesOnly: false,
    hideRejected: true,
    verifiedOnly: false,
    minPrice: undefined as number | undefined,
    maxPrice: undefined as number | undefined,
    rooms: undefined as number | undefined,
    analyzedOnly: false,
    hideShell: true,
    activeOnly: true,
    propertyType: undefined as string | undefined,  // "Новостройка", "Вторичка", or undefined
    isShell: undefined as boolean | undefined,      // true, false, or undefined for all
    minCondition: undefined as number | undefined,
    maxCondition: undefined as number | undefined,
  })

  const totalPages = computed(() => Math.ceil(totalListings.value / perPage.value))

  async function loadListings(page = 1) {
    const id = ++requestId
    controller?.abort()
    controller = new AbortController()
    loading.value = true
    error.value = null
    try {
      const response = await fetchListings({
        min_area: filters.value.minArea,
        move_in_ready: filters.value.moveInReady ? true : undefined,
        favorites_only: filters.value.favoritesOnly,
        hide_rejected: filters.value.hideRejected,
        verified_only: filters.value.verifiedOnly,
        page,
        per_page: perPage.value,
        min_price: filters.value.minPrice,
        max_price: filters.value.maxPrice,
        rooms: filters.value.rooms,
        analyzed_only: filters.value.analyzedOnly,
        hide_shell: filters.value.hideShell,
        active_only: filters.value.activeOnly,
        property_type: filters.value.propertyType,
        is_shell: filters.value.isShell,
        min_condition: filters.value.minCondition,
        max_condition: filters.value.maxCondition,
      }, controller.signal)
      if (id !== requestId) return
      listings.value = response.listings
      totalListings.value = response.total
      currentPage.value = response.page
    } catch (e) {
      if (id !== requestId) return
      if (e instanceof ApiError) {
        error.value = e.body?.error || `Ошибка ${e.status}: ${e.statusText}`
      } else if (e instanceof Error) {
        error.value = e.message
      } else {
        error.value = 'Не удалось загрузить объявления'
      }
      throw e  // Re-throw для обработки в компоненте
    } finally {
      if (id === requestId) loading.value = false
    }
  }

  async function loadStats() {
    statsLoading.value = true
    try {
      stats.value = await fetchStats()
    } catch (e) {
      if (e instanceof ApiError) {
        console.error('Failed to load stats:', e.status, e.body)
      } else {
        console.error('Failed to load stats:', e)
      }
      throw e  // Re-throw для обработки в компоненте
    } finally {
      statsLoading.value = false
    }
  }

  function setFilters(newFilters: Partial<typeof filters.value>, autoLoad = true) {
    filters.value = { ...filters.value, ...newFilters }
    if (autoLoad) {
      return loadListings(1)
    }
  }

  return {
    listings,
    stats,
    statsLoading,
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
