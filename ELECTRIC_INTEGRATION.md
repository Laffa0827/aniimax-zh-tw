# 電力系統整合說明

- RV 1–11：不啟用 Crackle Generator E-mode，計算維持原 Aniimax 邏輯。
- RV 12 以上：電力系統可用；新設定預設開啟 E-mode，使用者仍可手動關閉。
- Generator 等級由 RV 自動決定：RV12–13 Lv1、RV14–15 Lv2、RV16–17 Lv3、RV18–19 Lv4、RV20+ Lv5。
- 電力是共享電網：所有 E-mode 設施共用同一 Generator 容量。
- Crackle Generator 本身需要 1 隻 Lightning（電）屬性 Aniimo 全日駐守；目前模型為 1 台共享 Generator，因此啟用 E-mode 時額外計入 1 隻 Lightning Aniimo。
- 總需求不超過 120% Boost 門檻時，全網 120%；超過門檻但不超過 Generator 容量時，全網 100%；超過容量則不可行。
- E-mode 相容性由 electric_compatible() 控制。Farmland、Woodland、Tidewhisper Sandcastle、Dewy House、Nimbus Bed、Starfall Hammock、Floral Windmill、Heat Furnace、Cooling Unit、Sunlamp 不可電氣化；其他設施依使用者規則視為可電氣化。
- 耗電使用每座設施自己的 basePower + (level-1)*powerPerLevel 模型，不假設所有加工設施的起始瓦數相同。


## Current provisional power model

The calculator supports every RV level. Crackle Generator E-mode is unavailable below RV12 and becomes available at RV12.

Base power: Aniipod Maker, Mine, Well, and Dance Pad Polisher = 30W; all other E-mode-compatible facilities = 15W.
Per-level increment: Mine and Well = +30W/level; all other E-mode-compatible facilities = +15W/level.

These are the current provisional model values requested by the user and are intentionally not blocked on future reverse-engineering.
