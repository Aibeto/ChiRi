<script setup lang="ts">
import { ref, onMounted, computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { Bridge } from '@/utils/bridge';
import { getPackagesInfo, toast } from '@/kernelsu';
import { useSchedulerStore } from '@/stores/scheduler';

const { t } = useI18n();
const store = useSchedulerStore();

// pkg → appLabel 映射
const appLabelMap = ref<Record<string, string>>({});
const apps = ref<string[]>([]);
const searchText = ref('');
const showActionSheet = ref(false);
const selectedPkg = ref('');
// 扫描状态：扫描期间禁用按钮，防止多线程同时扫描（getInstalledApps 重复触发）
const isScanning = ref(false);

// 动作单：仅 Chiri 设备上，白名单应用在标准模式之前追加专属特调选项
// （只读白名单提供，用户不可增改）；Yumi 设备无特调选项。
const actions = computed(() => {
  const list: any[] = [];
  const special = store.isChiri && selectedPkg.value ? store.specialTuned[selectedPkg.value] : undefined;
  if (special) {
    special.modes.forEach(modeKey => list.push({
      name: `${t('special_tuned')}：${modeKey}`,
      subname: t('desc_special_tuned'),
      color: '#9C27B0',
      modeKey,
      isSpecial: true
    }));
  }
  list.push(
    { name: t('mode_powersave'), subname: t('desc_powersave'), color: '#4CAF50', modeKey: 'powersave' },
    { name: t('mode_balance'), subname: t('desc_balance'), color: '#2196F3', modeKey: 'balance' },
    { name: t('mode_performance'), subname: t('desc_performance'), color: '#FF9800', modeKey: 'performance' },
    { name: t('mode_fast'), subname: t('desc_fast'), color: '#F44336', modeKey: 'fast' },
    // { name: t('mode_fas'), subname: t('desc_fas'), color: '#E91E63', modeKey: 'fas' }, // FAS 暂禁用
    { name: t('delete_rule'), color: '#FF0000', isDelete: true }
  );
  return list;
});

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

// 特调标签：显示具体模式内容（如“特调：akmode”）。
// 用户自定义过特调模式时优先显示自定义值，否则显示白名单优先回退模式。
const specialLabel = (pkg: string) => {
  const entry = store.specialTuned[pkg];
  if (!entry) return t('special_tuned');
  const custom = store.appRules[pkg];
  if (custom && entry.modes.includes(custom)) return `${t('special_tuned')}：${custom}`;
  return `${t('special_tuned')}：${entry.fallback}`;
};

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
// 扫描完成后，若处于 Chiri 模式，清理 rules.yaml 中非白名单/非法的特调映射（与后端门控一致）
const onRescan = async () => {
  if (isScanning.value) return;
  isScanning.value = true;
  try {
    await refreshAppList();
    let removed = 0;
    if (store.isChiri) {
      removed = await Bridge.pruneSpecialTunedRules(store.specialTuned);
    }
    await store.initData();
    toast(t('rescan_done') + (removed > 0 ? t('illegal_special_removed', { count: removed }) : ''));
  } catch (e) {
    toast(t('rescan_failed'));
  } finally {
    isScanning.value = false;
  }
};

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

const openMenu = (pkg: string) => {
  selectedPkg.value = pkg;
  showActionSheet.value = true;
};

const onSelectAction = async (item: any) => {
  showActionSheet.value = false;
  if (item.isDelete) {
    delete store.appRules[selectedPkg.value];
    await Bridge.saveAppRule(selectedPkg.value, '');
  } else {
    store.appRules[selectedPkg.value] = item.modeKey;
    await Bridge.saveAppRule(selectedPkg.value, item.modeKey);
  }
};
</script>

<template>
  <div class="app-rules">
    <van-nav-bar :title="t('app_management')" left-arrow @click-left="$router.back()" fixed placeholder>
      <template #right>
        <span
          class="rescan-btn"
          :class="{ disabled: isScanning }"
          @click="onRescan"
        >{{ isScanning ? t('scanning') : t('rescan') }}</span>
      </template>
    </van-nav-bar>

    <van-search v-model="searchText" :placeholder="t('search_apps')" />

    <van-list>
      <van-cell
        v-for="pkg in filteredApps"
        :key="pkg"
        :title="getLabel(pkg)"
        :label="pkg"
        center
        clickable
        @click="openMenu(pkg)"
      >
        <template #icon>
          <img
            :src="`ksu://icon/${pkg}`"
            style="width: 40px; height: 40px; margin-right: 12px; border-radius: 8px;"
            loading="lazy"
          />
        </template>
        <template #value>
          <div class="mode-tags">
            <!-- 内部特调白名单（只读，仅 Chiri 设备）：常驻显示“特调”标签并展示具体模式内容 -->
            <van-tag v-if="store.isChiri && store.specialTuned[pkg]" type="warning" size="medium" plain>
              {{ specialLabel(pkg) }}
            </van-tag>
            <!-- 用户自定义配置优先：显示自定义模式的原标签（与特调标签并存） -->
            <van-tag v-if="store.appRules[pkg]" type="primary" size="medium">
              {{ modeLabel(store.appRules[pkg]) }}
            </van-tag>
            <span v-if="!store.appRules[pkg] && !(store.isChiri && store.specialTuned[pkg])" class="no-rule">{{ t('not_configured') }}</span>
          </div>
        </template>
      </van-cell>
    </van-list>

    <van-action-sheet
      v-model:show="showActionSheet"
      :actions="actions"
      :description="`${t('select_mode_for')} ${getLabel(selectedPkg)}`"
      :cancel-text="t('cancel')"
      @select="onSelectAction"
    />
  </div>
</template>

<style scoped>
.no-rule { font-size: 12px; color: #bbb; }
.mode-tags { display: flex; align-items: center; gap: 4px; }
.rescan-btn { font-size: 14px; color: #1989fa; cursor: pointer; }
.rescan-btn.disabled { color: #c8c9cc; pointer-events: none; }
</style>
