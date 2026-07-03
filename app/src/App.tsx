import { useCallback, useEffect, useState } from "react";
import { api } from "./api";
import type {
  Account,
  CategorySpend,
  MerchantSpend,
  MonthlyFlow,
  Subscription,
  Transaction,
} from "./types";
import { TopBar } from "./components/TopBar";
import { MonthlyFlowCard } from "./components/MonthlyFlow";
import { SpendByCategory } from "./components/SpendByCategory";
import { MerchantLeaderboard } from "./components/MerchantLeaderboard";
import { Subscriptions } from "./components/Subscriptions";
import { TransactionList } from "./components/TransactionList";
import { Toast, type ToastMsg } from "./components/Toast";

export interface Dashboard {
  accounts: Account[];
  flow: MonthlyFlow[];
  categories: CategorySpend[];
  merchants: MerchantSpend[];
  subscriptions: Subscription[];
  transactions: Transaction[];
  vocab: string[];
}

const EMPTY: Dashboard = {
  accounts: [],
  flow: [],
  categories: [],
  merchants: [],
  subscriptions: [],
  transactions: [],
  vocab: [],
};

export default function App() {
  const [data, setData] = useState<Dashboard>(EMPTY);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [toast, setToast] = useState<ToastMsg | null>(null);

  const notify = useCallback((msg: ToastMsg) => {
    setToast(msg);
    window.setTimeout(() => setToast(null), 3200);
  }, []);

  const reload = useCallback(async () => {
    try {
      const [accounts, flow, categories, merchants, subscriptions, transactions, vocab] =
        await Promise.all([
          api.accounts(),
          api.flow(),
          api.spendCategories(),
          api.merchants(undefined, 15),
          api.subscriptions(),
          api.transactions(),
          api.categories(),
        ]);
      setData({ accounts, flow, categories, merchants, subscriptions, transactions, vocab });
    } catch (err) {
      notify({ kind: "err", text: `Load failed: ${String(err)}` });
    } finally {
      setLoading(false);
    }
  }, [notify]);

  useEffect(() => {
    void reload();
  }, [reload]);

  const onCategorize = useCallback(async () => {
    setBusy(true);
    try {
      const r = await api.categorize();
      notify({
        kind: "ok",
        text: `Categorized ${r.rule} by rule · ${r.remaining} still uncategorized`,
      });
      await reload();
    } catch (err) {
      notify({ kind: "err", text: `Categorize failed: ${String(err)}` });
    } finally {
      setBusy(false);
    }
  }, [notify, reload]);

  const onCategorizeLlm = useCallback(async () => {
    setBusy(true);
    try {
      // Rule pass first (cheap), then the LLM tail over what's left.
      const r = await api.categorize();
      const m = await api.categorizeLlm();
      notify({ kind: "ok", text: `Categorized ${r.rule} by rule · ${m} by AI` });
      await reload();
    } catch (err) {
      notify({ kind: "err", text: `AI categorize failed: ${String(err)}` });
    } finally {
      setBusy(false);
    }
  }, [notify, reload]);

  const onPull = useCallback(async () => {
    setBusy(true);
    try {
      const r = await api.pullLive();
      notify({
        kind: "ok",
        text: `Pulled ${r.accounts} account(s): ${r.added} added, ${r.updated} updated${
          r.warnings.length ? ` · ${r.warnings.length} warning(s)` : ""
        }`,
      });
      await reload();
    } catch (err) {
      notify({ kind: "err", text: `Pull failed: ${String(err)}` });
    } finally {
      setBusy(false);
    }
  }, [notify, reload]);

  const onClaim = useCallback(
    async (token: string) => {
      setBusy(true);
      try {
        const msg = await api.claim(token);
        notify({ kind: "ok", text: msg });
      } catch (err) {
        notify({ kind: "err", text: `Connect failed: ${String(err)}` });
      } finally {
        setBusy(false);
      }
    },
    [notify]
  );

  return (
    <div className="app">
      <TopBar
        accounts={data.accounts}
        busy={busy || loading}
        onPull={onPull}
        onCategorize={onCategorize}
        onCategorizeLlm={onCategorizeLlm}
        onClaim={onClaim}
        onRefresh={reload}
      />

      {loading ? (
        <div className="muted">Loading…</div>
      ) : (
        <div className="grid">
          <MonthlyFlowCard flow={data.flow} />
          <SpendByCategory rows={data.categories} />
          <MerchantLeaderboard rows={data.merchants} />
          <Subscriptions rows={data.subscriptions} />
          <TransactionList
            txns={data.transactions}
            vocab={data.vocab}
            onChanged={reload}
            notify={notify}
          />
        </div>
      )}

      {toast && <Toast msg={toast} />}
    </div>
  );
}
