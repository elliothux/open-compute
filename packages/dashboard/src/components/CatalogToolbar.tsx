import { ArrowClockwise } from "@phosphor-icons/react";
import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";

interface CatalogToolbarProps {
  search?: string;
  onSearchChange?: (value: string) => void;
  searchPlaceholder?: string;
  onRefresh?: () => void;
  isRefreshing?: boolean;
  filters?: React.ReactNode;
  primaryAction?: React.ReactNode;
}

export function CatalogToolbar({
  search,
  onSearchChange,
  searchPlaceholder = "Search…",
  onRefresh,
  isRefreshing = false,
  filters,
  primaryAction,
}: CatalogToolbarProps) {
  return (
    <div className="mb-4 flex flex-wrap items-center gap-3">
      {onSearchChange ? (
        <div className="min-w-[14rem] flex-1">
          <Input
            className="w-full"
            value={search ?? ""}
            onChange={event => onSearchChange(event.target.value)}
            placeholder={searchPlaceholder}
            aria-label="Search catalog"
          />
        </div>
      ) : null}
      {filters}
      <div className="ml-auto flex items-center gap-2">
        {onRefresh ? (
          <Button
            variant="secondary"
            disabled={isRefreshing}
            onClick={onRefresh}
            aria-label="Refresh catalog"
            icon={<ArrowClockwise size={16} className={isRefreshing ? "animate-spin" : undefined} />}
          >
            Refresh
          </Button>
        ) : null}
        {primaryAction}
      </div>
    </div>
  );
}
