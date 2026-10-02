import React from "react";
import { Navigate, useLocation } from "react-router-dom";

// /plans was a static copy of the plan picker that couldn't sell anything.
// Plans are bought on /billing (signed out it shows the same cards and signs
// you in); keep ?plan= so a chosen plan still starts checkout.
export function PlansPage() {
  const location = useLocation();
  return <Navigate to={`/billing${location.search}`} replace />;
}
