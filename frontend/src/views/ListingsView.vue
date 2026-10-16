<script setup lang="ts">
import { onMounted, onUnmounted, ref, computed, watch } from 'vue'
import { useRouter, useRoute } from 'vue-router'
import {
  NGrid,
  NGi,
  NSpin,
  NPagination,
  NSpace,
  NCard,
  NInputNumber,
  NButton,
  NSwitch,
  NEmpty,
  NSelect,
  NSlider,
  NAlert,
  NTag,
  NDescriptions,
  NDescriptionsItem,
  NProgress,
  NDivider,
  useMessage,
} from 'naive-ui'
import { useListingsStore } from '@/stores/listings'
import ListingCard from '@/components/ListingCard.vue'
import { fetchListingImages, triggerCollection, ApiError, type JobStatus, type JobPhase } from '@/api'

const store = useListingsStore()
const router = useRouter()
const route = useRoute()
const message = useMessage()

// Cache first image for each listing
const listingImages = ref<Map<number, number | null>>(new Map())

// Collection state
const isCollecting = ref(false)
const collectionError = ref<string | null>(null)
const backendError = ref<string | null>(null)
const jobStatus = ref<JobStatus | null>(null)

// WebSocket connection for real-time status updates
let statusWs: WebSocket | null = null
let wsReconnectTimeout: ReturnType<typeof setTimeout> | null = null
const wsConnected = ref(false)
let disposed = false
let imageRequest = 0

function getWsUrl(): string {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${protocol}//${window.location.host}/api/ws/status`
}

function connectWebSocket() {
  if (disposed || (statusWs && statusWs.readyState < WebSocket.CLOSING)) return

  statusWs = new WebSocket(getWsUrl())

  statusWs.onopen = () => {
    wsConnected.value = true
    backendError.value = null
  }

  statusWs.onmessage = (event) => {
    try {
      const status: JobStatus = JSON.parse(event.data)
      jobStatus.value = status
      collectionError.value = status.last_error
      const wasCollecting = isCollecting.value
      isCollecting.value = status.phase !== 'idle'

      // If collection just finished, reload data
      if (wasCollecting && !isCollecting.value) {
        Promise.all([store.loadStats(), store.loadListings(store.currentPage)]).then(() => {
          loadFirstImages()
          if (status.last_error) message.error(status.last_error)
          else message.success('Задача завершена')
        }).catch(() => {
          backendError.value = 'Не удалось обновить данные'
        })
      }
    } catch (e) {
      console.error('Failed to parse WebSocket message:', e)
    }
  }

  statusWs.onclose = () => {
    wsConnected.value = false
    // Reconnect after 3 seconds
    if (!disposed) wsReconnectTimeout = setTimeout(connectWebSocket, 3000)
  }

  statusWs.onerror = () => {
    backendError.value = 'Не удалось подключиться к серверу'
  }
}

function disconnectWebSocket() {
  if (wsReconnectTimeout) {
    clearTimeout(wsReconnectTimeout)
    wsReconnectTimeout = null
  }
  if (statusWs) {
    statusWs.onclose = null
    statusWs.onmessage = null
    statusWs.onerror = null
    statusWs.close()
    statusWs = null
  }
}

const phaseLabels: Record<JobPhase, string> = {
  idle: 'Готов',
  collecting_listings: 'Сбор объявлений',
  downloading_images: 'Загрузка изображений',
  analyzing_images: 'AI анализ изображений',
  analyzing_listings: 'AI анализ объявлений',
  syncing_actuality: 'Проверка актуальности',
}

const currentPhaseLabel = computed(() => {
  if (!jobStatus.value || jobStatus.value.phase === 'idle') return null
  return phaseLabels[jobStatus.value.phase]
})

const progressText = computed(() => {
  if (!jobStatus.value?.progress) return null
  const { processed, total } = jobStatus.value.progress
  if (total) {
    return `${processed} / ${total}`
  }
  return `${processed}`
})

const progressPercentage = computed(() => {
  if (!jobStatus.value?.progress) return null
  const { processed, total } = jobStatus.value.progress
  if (total && total > 0) {
    return Math.round((processed / total) * 100)
  }
  return null
})

const connectionStatus = computed(() => wsConnected.value ? 'Подключен' : 'Отключен')

async function startCollection() {
  collectionError.value = null

  try {
    const result = await triggerCollection()

    if (result.error === 'already_running') {
      message.warning('Задача уже выполняется')
    } else {
      message.success('Сбор объявлений запущен')
    }
    // Status updates will come via WebSocket automatically
  } catch (e) {
    if (e instanceof ApiError) {
      collectionError.value = e.body?.error || `Ошибка: ${e.status} ${e.statusText}`
    } else if (e instanceof Error) {
      collectionError.value = e.message
    } else {
      collectionError.value = 'Неизвестная ошибка'
    }
    message.error(collectionError.value!)
  }
}

const collectionButtonText = computed(() => {
  if (isCollecting.value && currentPhaseLabel.value) {
    return currentPhaseLabel.value
  }
  if (isCollecting.value) return 'Загрузка...'
  if (collectionError.value) return 'Повторить'
  return 'Обновить объявления'
})

async function loadFirstImages() {
  const request = ++imageRequest
  const listings = [...store.listings]
  listingImages.value = new Map()
  for (let offset = 0; offset < listings.length; offset += 6) {
    await Promise.all(listings.slice(offset, offset + 6).map(async listing => {
      try {
        const images = await fetchListingImages(listing.id)
        if (disposed || request !== imageRequest) return
        const image = images.find(img => img.s3_key && img.status.toLowerCase() === 'downloaded')
        listingImages.value.set(listing.id, image?.id ?? null)
      } catch { /* The card keeps its no-photo placeholder. */ }
    }))
    if (disposed || request !== imageRequest) return
  }
}

async function loadFromRoute() {
  store.setFilters({
    minPrice: getQueryNumber('minPrice'), maxPrice: getQueryNumber('maxPrice'), rooms: getQueryNumber('rooms'),
    minArea: getQueryNumber('minArea'), moveInReady: getQueryBool('moveInReady', false),
    verifiedOnly: getQueryBool('verifiedOnly', false), favoritesOnly: getQueryBool('favoritesOnly', false), hideRejected: getQueryBool('hideRejected', true),
    analyzedOnly: getQueryBool('analyzedOnly', false), hideShell: getQueryBool('hideShell', true),
    activeOnly: getQueryBool('activeOnly', true), propertyType: getQueryString('propertyType') || undefined,
    isShell: getQueryString('shellFilter') === '' ? undefined : getQueryString('shellFilter') === 'without',
    minCondition: getQueryNumber('minCondition'), maxCondition: getQueryNumber('maxCondition'),
  }, false)
  try {
    await store.loadListings(Math.max(1, getQueryNumber('page') ?? 1))
    await loadFirstImages()
  } catch { /* The store exposes the error in the page. */ }
}
onMounted(() => {
  connectWebSocket()
  store.loadStats().catch(() => { backendError.value = 'Не удалось загрузить статистику' })
})
function syncFormFromRoute() {
  minPrice.value = getQueryNumber('minPrice'); maxPrice.value = getQueryNumber('maxPrice'); rooms.value = getQueryNumber('rooms')
  minArea.value = getQueryNumber('minArea'); moveInReady.value = getQueryBool('moveInReady', false)
  verifiedOnly.value = getQueryBool('verifiedOnly', false)
  favoritesOnly.value = getQueryBool('favoritesOnly', false); hideRejected.value = getQueryBool('hideRejected', true)
  analyzedOnly.value = getQueryBool('analyzedOnly', false); hideShell.value = getQueryBool('hideShell', true)
  activeOnly.value = getQueryBool('activeOnly', true); propertyType.value = getQueryString('propertyType')
  shellFilter.value = getQueryString('shellFilter'); conditionRange.value = [getQueryNumber('minCondition') ?? 0, getQueryNumber('maxCondition') ?? 100]
}
watch(() => route.query, () => { syncFormFromRoute(); void loadFromRoute() })

onUnmounted(() => {
  disposed = true
  imageRequest++
  disconnectWebSocket()
})

async function handlePageChange(page: number) {
  // Обновляем URL при смене страницы
  router.replace({ query: { ...route.query, page: page.toString() } })

}

function handleListingClick(listing: any) {
  router.push({ name: 'listing-detail', params: { id: listing.id } })
}

// Filter form - инициализация из URL
function getQueryNumber(key: string): number | undefined {
  const val = route.query[key]
  if (val && typeof val === 'string') {
    const num = Number(val)
    return Number.isFinite(num) ? num : undefined
  }
  return undefined
}

function getQueryBool(key: string, defaultVal: boolean): boolean {
  const val = route.query[key]
  if (val === 'true') return true
  if (val === 'false') return false
  return defaultVal
}

function getQueryString(key: string, defaultVal: string = ''): string {
  const val = route.query[key]
  return typeof val === 'string' ? val : defaultVal
}

const minArea = ref<number | undefined>(getQueryNumber('minArea'))
const moveInReady = ref(getQueryBool('moveInReady', false))
const verifiedOnly = ref(getQueryBool('verifiedOnly', false))
const favoritesOnly = ref(getQueryBool('favoritesOnly', false))
const hideRejected = ref(getQueryBool('hideRejected', true))
const minPrice = ref<number | undefined>(getQueryNumber('minPrice'))
const maxPrice = ref<number | undefined>(getQueryNumber('maxPrice'))
const rooms = ref<number | undefined>(getQueryNumber('rooms'))
const analyzedOnly = ref(getQueryBool('analyzedOnly', false))
const hideShell = ref(getQueryBool('hideShell', true))
const activeOnly = ref(getQueryBool('activeOnly', true))
const propertyType = ref<string>(getQueryString('propertyType'))
const shellFilter = ref<string>(getQueryString('shellFilter'))
const conditionRange = ref<[number, number]>([
  getQueryNumber('minCondition') ?? 0,
  getQueryNumber('maxCondition') ?? 100
])

// Select options
const propertyTypeOptions = [
  { label: 'Все', value: '' },
  { label: 'Новостройка', value: 'Новостройка' },
  { label: 'Вторичка', value: 'Вторичка' },
]

const shellOptions = [
  { label: 'Все', value: '' },
  { label: 'С отделкой', value: 'with' },
  { label: 'Без отделки (shell)', value: 'without' },
]

async function applyFilters() {
  // Сохраняем фильтры в URL
  const query: Record<string, string> = { page: '1' }
  if (minPrice.value) query.minPrice = minPrice.value.toString()
  if (maxPrice.value) query.maxPrice = maxPrice.value.toString()
  if (rooms.value) query.rooms = rooms.value.toString()
  if (analyzedOnly.value) query.analyzedOnly = 'true'
  query.hideShell = String(hideShell.value && shellFilter.value !== 'without')
  if (minArea.value != null) query.minArea = String(minArea.value)
  if (moveInReady.value) query.moveInReady = 'true'
  if (verifiedOnly.value) query.verifiedOnly = 'true'
  if (favoritesOnly.value) query.favoritesOnly = 'true'
  query.hideRejected = String(hideRejected.value)
  if (!activeOnly.value) query.activeOnly = 'false'  // только если false (не дефолт)
  if (propertyType.value) query.propertyType = propertyType.value
  if (shellFilter.value) query.shellFilter = shellFilter.value
  if (conditionRange.value[0] > 0) query.minCondition = conditionRange.value[0].toString()
  if (conditionRange.value[1] < 100) query.maxCondition = conditionRange.value[1].toString()

  const previousPath = route.fullPath
  await router.replace({ query })
  if (route.fullPath === previousPath) await loadFromRoute()

}

async function clearFilters() {
  await router.replace({ query: { page: '1' } })
  syncFormFromRoute()
}
void loadFromRoute()
</script>

<template>
  <n-space vertical size="large">
    <!-- Server Status Card -->
    <n-card size="small">
      <template #header>
        <n-space align="center" justify="space-between" style="width: 100%">
          <n-space align="center">
            <span style="font-weight: 500;">Сервер</span>
            <n-tag :type="wsConnected ? 'success' : 'error'" size="small">
              {{ connectionStatus }}
            </n-tag>
          </n-space>
          <n-button
            :type="collectionError ? 'error' : 'primary'"
            :loading="isCollecting"
            :disabled="backendError !== null || !wsConnected"
            size="small"
            @click="startCollection"
          >
            {{ collectionButtonText }}
          </n-button>
        </n-space>
      </template>

      <!-- Error Alert -->
      <n-alert v-if="backendError || store.error" type="error" style="margin-bottom: 12px">
        {{ backendError || store.error }}
      </n-alert>

      <n-alert v-if="collectionError" type="error" style="margin-bottom: 12px">{{ collectionError }}</n-alert>
      <!-- Current Job Progress -->
      <div v-if="isCollecting" style="margin-bottom: 12px">
        <n-space align="center" justify="space-between" style="margin-bottom: 4px">
          <span>{{ currentPhaseLabel }}</span>
          <span v-if="progressText" style="color: #888">{{ progressText }}</span>
        </n-space>
        <n-progress
          type="line"
          :percentage="progressPercentage ?? 100"
          :show-indicator="false"
          :status="progressPercentage !== null ? 'default' : 'info'"
          :processing="progressPercentage === null"
        />
      </div>

      <!-- Stats -->
      <n-descriptions v-if="store.stats" :column="4" label-placement="top" size="small">
        <n-descriptions-item label="В подборе / всего">
          {{ store.stats.active_listings }} / {{ store.stats.total_listings }}
        </n-descriptions-item>
        <n-descriptions-item label="Изображений">
          {{ store.stats.images.downloaded }}
          <span v-if="store.stats.images.pending > 0" style="color: #888"> (+{{ store.stats.images.pending }})</span>
        </n-descriptions-item>
        <n-descriptions-item label="Анализ изображений">
          {{ store.stats.analysis.images_analyzed }}
          <span v-if="store.stats.analysis.images_pending > 0" style="color: #888"> (+{{ store.stats.analysis.images_pending }})</span>
        </n-descriptions-item>
        <n-descriptions-item label="Анализ объявлений">
          {{ store.stats.analysis.listings_analyzed }}
          <span v-if="store.stats.analysis.listings_pending > 0" style="color: #888"> (+{{ store.stats.analysis.listings_pending }})</span>
        </n-descriptions-item>
      </n-descriptions>

      <!-- No stats fallback -->
      <n-spin v-else-if="store.statsLoading" size="small" />
      <n-alert v-else-if="!backendError" type="warning">
        Не удалось загрузить статистику
      </n-alert>
      <n-space v-if="store.stats" vertical size="small" style="margin-top: 12px">
        <n-space>
          <n-tag type="success">Свежее подтверждение: {{ store.stats.confirmed_listings }}</n-tag>
          <n-tag type="warning">Без свежего подтверждения: {{ store.stats.unconfirmed_listings }}</n-tag>
          <n-tag>Снято по последним сведениям: {{ store.stats.removed_listings }}</n-tag>
        </n-space>
        <div v-for="source in store.stats.availability.filter(s => s.retry_after && Date.parse(s.retry_after) > Date.now())" :key="source.source" style="color: #888; font-size: 0.85em">
          {{ source.source }}: проверка приостановлена до {{ new Date(source.retry_after!).toLocaleString('ru-RU') }}. {{ source.last_error }}
        </div>
        <div style="color: #888; font-size: 0.85em">Наличие в подборе не подтверждает, что квартира ещё продаётся. Подтверждение актуальности действует ограниченное время.</div>
      </n-space>


    </n-card>

    <!-- Filters -->
    <n-card title="Фильтры" size="small">
      <n-space vertical>
        <!-- Row 1: Price, Rooms, Property Type -->
        <n-space align="center" wrap>
          <n-input-number
            v-model:value="minPrice"
            placeholder="Цена от"
            :min="0"
            :step="100000"
            clearable
            style="width: 140px"
          />
          <n-input-number
            v-model:value="maxPrice"
            placeholder="Цена до"
            :min="0"
            :step="100000"
            clearable
            style="width: 140px"
          />
          <n-input-number
            v-model:value="rooms"
            placeholder="Комнат"
            :min="1"
            :max="10"
            clearable
            style="width: 100px"
          />
          <n-select
            v-model:value="propertyType"
            :options="propertyTypeOptions"
            placeholder="Тип"
            style="width: 140px"
          />
          <n-select
            v-model:value="shellFilter"
            :options="shellOptions"
            placeholder="Отделка"
            style="width: 160px"
          />
        </n-space>

        <n-space align="center" wrap>
          <n-input-number v-model:value="minArea" placeholder="Площадь от, м²" :min="0" clearable style="width: 170px" />
          <n-space align="center"><n-switch v-model:value="moveInReady" /><span>Можно заселяться по анализу</span></n-space>
          <n-space align="center"><n-switch v-model:value="verifiedOnly" /><span>Только с недавним подтверждением актуальности</span></n-space>
          <n-space align="center"><n-switch v-model:value="favoritesOnly" /><span>Избранное</span></n-space>
          <n-space align="center"><n-switch v-model:value="hideRejected" /><span>Исключить скрытые</span></n-space>
        </n-space>
        <!-- Row 2: Condition Range -->
        <n-space align="center" style="width: 100%">
          <span style="white-space: nowrap">Состояние: {{ conditionRange[0] }} - {{ conditionRange[1] }}</span>
          <n-slider
            v-model:value="conditionRange"
            range
            :min="0"
            :max="100"
            :step="5"
            style="width: 300px"
          />
        </n-space>

        <!-- Row 3: Switches and Buttons -->
        <n-space align="center" wrap>
          <n-space align="center">
            <n-switch v-model:value="analyzedOnly" />
            <span>Только с анализом</span>
          </n-space>
          <n-space align="center">
            <n-switch v-model:value="hideShell" :disabled="shellFilter === 'without'" />
            <span>Скрыть без отделки</span>
          </n-space>
          <n-space align="center">
            <n-switch v-model:value="activeOnly" />
            <span>Только активные</span>
          </n-space>
          <n-button type="primary" @click="applyFilters">Применить</n-button>
          <n-button @click="clearFilters">Сбросить</n-button>
        </n-space>
      </n-space>
    </n-card>

    <!-- Listings Grid -->
    <n-spin :show="store.loading">
      <n-grid v-if="store.listings.length > 0" cols="1 s:2 m:3 l:4" :x-gap="16" :y-gap="16" responsive="screen">
        <n-gi v-for="listing in store.listings" :key="listing.id">
          <ListingCard
            :listing="listing"
            :first-image-id="listingImages.get(listing.id)"
            @click="handleListingClick"
          />
        </n-gi>
      </n-grid>
      <n-empty v-else-if="!store.loading" description="Объявления не найдены" />
    </n-spin>

    <!-- Pagination -->
    <n-space justify="center" v-if="store.totalPages > 1">
      <n-pagination
        :page="store.currentPage"
        :page-count="store.totalPages"
        :page-slot="7"
        @update:page="handlePageChange"
      />
    </n-space>
  </n-space>
</template>
