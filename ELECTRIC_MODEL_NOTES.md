# Crackle Generator E-mode 計算模型

這個版本保留 Aniimax 原本的聯合最佳化模型，電力只是新增一組共享電網約束與 E-mode 決策，不改變原本的配方、物料平衡、設施整數分配、環境覆蓋、模組與目標優先順序邏輯。

## Generator

| Generator | Capacity | 120% Boost 門檻 |
|---|---:|---:|
| Lv.1 | 600W | 500W |
| Lv.2 | 800W | 660W |
| Lv.3 | 1000W | 800W |
| Lv.4 | 1200W | 1000W |
| Lv.5 | 1500W | 1200W |

RV 自動對應：RV12–13 Lv.1、RV14–15 Lv.2、RV16–17 Lv.3、RV18–19 Lv.4、RV20+ Lv.5。

## Shared grid

每個啟用 E-mode 的設施單位都會佔用完整 `electricRequire`。

- 總需求 <= Generator capacity：電網可運作。
- 總需求 <= 120% Boost 門檻：整個 E-mode 電網為 120%。
- 總需求 > Boost 門檻但 <= Capacity：整個 E-mode 電網為 100%。
- 總需求 > Capacity：不可行。

因此不會把每座設施各自當成一個獨立的 120% 電源。

## Exact optimizer

對每個可確認 E-mode 的非輪替設施，模型同時決定：

- 實際使用的設施單位數
- E-mode 單位數
- 是否位於 120% Boost 電網
- 生產速率

若進入 120% Boost 電網：

`rate * production_time <= normal_units + 0.2 * powered_units`

若電網超過 Boost 門檻：

`rate * production_time <= normal_units`

但 E-mode 單位仍可保留並消耗 Generator 容量，因為 E-mode 在 100% 時仍可用於自動化／免人工工作；目前在沒有實際 Aniimo roster 限制時，最佳化器不會為了「自動化」而犧牲收益。

## Aniimo staffing

若輸入有實際 Aniimo crew，E-mode 單位會從對應工作者的 busy time 中扣除：

- 100% E-mode：每個 powered unit 減少 1 個工作單位
- 120% E-mode：每個 boosted unit 減少 1.2 個工作單位

因此電力會同時影響「產能」與「人工需求」。

## 未確認資料

尚未有足夠直接證據的設施不會自行填入耗電值；`electric_require()` 對這些設施回傳 `None`，避免最佳化器使用臆測資料。


## ElectricRequire progression rule (user-confirmed)

- Mine: level × 30W.
- Well: level × 30W. Directly confirmed by the user's Well Lv4 screenshot = 120W.
- Regular processing facilities: level × 15W.
- Dance Pad Polisher / Aniipod Maker remain special facilities with their separately confirmed 30W-per-level progression.
- Electric compatibility is independent from electricRequire.
