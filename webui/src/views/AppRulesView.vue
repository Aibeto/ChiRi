<script setup lang="ts">
// AppRulesView.vue: [state] [modes-ro] [scan] [filter]
import { ref, onMounted, computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { Bridge } from '@/utils/bridge';
import { getPackagesInfo, toast } from '@/kernelsu';
import { useSchedulerStore } from '@/stores/scheduler';

const { t } = useI18n();
const store = useSchedulerStore();

// [state] 
// pkg → appLabel 映射
const appLabelMap = ref<Record<string, string>>({});
const apps = ref<string[]>([]);
const searchText = ref('');
// 扫描状态：扫描期间禁用按钮，防止多线程同时扫描（getInstalledApps 重复触发）
const isScanning = ref(false);

// [modes-ro] 
// 应用性能模式已禁止在 WebUI 指定/修改（rules.yaml 只读，由模块维护）。
// 本页仅只读展示规则标签：特调 / FAS 白名单 + rules.yaml 现存 app_modes。
const modeLabel = (modeKey: string) => {
  switch (modeKey) {
    case 'powersave': return t('mode_powersave');
    case 'balance': return t('mode_balance');
    case 'performance': return t('mode_performance');
    case 'fast': return t('mode_fast');
    // case 'fas': return t('mode_fas'); // FAS 暂禁用
    default: return modeKey;
  }
};

// 特调标签：只读标注白名单应用的内置特调模式（仅 Chiri 设备显示）。
// 用户不可增改特调，直接展示白名单优先回退模式。
const specialLabel = (pkg: string) => {
  const entry = store.specialTuned[pkg];
  return entry ? `${t('special_tuned')}：${entry.fallback}` : t('special_tuned');
};

// [scan] 
const refreshAppList = async () => {
  const packages = await Bridge.getInstalledApps();
  apps.value = packages;
  // 应用信息获取失败时降级显示包名，不影响主流程
  try {
    const infos = getPackagesInfo(packages);
    infos.forEach(info => {
      appLabelMap.value[info.packageName] = info.appLabel;
    });
  } catch (e) { /* 降级为包名 */ }
};

onMounted(async () => {
  await refreshAppList();
  await store.initData();
});

// 手动扫描：仅允许单实例运行（isScanning 互斥）。
const onRescan = async () => {
  if (isScanning.value) return;
  isScanning.value = true;
  try {
    await refreshAppList();
    await store.initData();
    toast(t('rescan_done'));
  } catch (e) {
    toast(t('rescan_failed'));
  } finally {
    isScanning.value = false;
  }
};

// [filter] 
// 用应用名或包名都能搜到
const filteredApps = computed(() => {
  const q = searchText.value.toLowerCase();
  if (!q) return apps.value;
  return apps.value.filter(pkg =>
    pkg.toLowerCase().includes(q) ||
    (appLabelMap.value[pkg] || '').toLowerCase().includes(q)
  );
});

// 优先显示应用名，缺失时降级为包名
const getLabel = (pkg: string) => appLabelMap.value[pkg] || pkg;
</script>

<template>
  <div class="app-rules">
    <van-nav-bar :title="t('app_management')" left-arrow @click-left="$router.back()" fixed placeholder>
      <template #right>
        <span class="rescan-btn" :class="{ disabled: isScanning }" @click="onRescan">{{ isScanning ? t('scanning') :
          t('rescan') }}</span>
      </template>
    </van-nav-bar>

    <van-search v-model="searchText" :placeholder="t('search_apps')" />

    <van-list>
      <van-cell v-for="pkg in filteredApps" :key="pkg" :title="getLabel(pkg)" :label="pkg" center>
        <template #icon>
          <img :src="`ksu://icon/${pkg}`" style="width: 40px; height: 40px; margin-right: 12px; border-radius: 8px;"
            loading="lazy" />
        </template>
        <template #value>
          <div class="mode-tags">
            <!-- 内部特调白名单（只读标注，仅 Chiri 设备）：常驻显示"特调"标签与内置模式 -->
            <van-tag v-if="store.isChiri && store.specialTuned[pkg]" type="warning" size="medium" plain>
              {{ specialLabel(pkg) }}
            </van-tag>
            <!-- FAS 白名单（只读标注，仅 Chiri 设备）：命中的应用由守护进程 FAS 接管 -->
            <van-tag v-if="store.isChiri && store.fasWhitelist[pkg]" type="primary" size="medium" plain>FAS</van-tag>
            <!-- rules.yaml 现存 app_modes：只读展示，不提供修改入口 -->
            <van-tag v-if="store.appRules[pkg]" type="primary" size="medium">
              {{ modeLabel(store.appRules[pkg]) }}
            </van-tag>
            <span v-if="!store.appRules[pkg] && !(store.isChiri && store.specialTuned[pkg]) && !store.fasWhitelist[pkg]"
              class="no-rule">{{ t('not_configured') }}</span>
          </div>
        </template>
      </van-cell>
    </van-list>
  </div>
</template>

<style scoped>
.no-rule {
  font-size: 12px;
  color: #bbb;
}

.mode-tags {
  display: flex;
  align-items: center;
  gap: 4px;
}

.rescan-btn {
  font-size: 14px;
  color: #1989fa;
  cursor: pointer;
}

.rescan-btn.disabled {
  color: #c8c9cc;
  pointer-events: none;
}
</style>
