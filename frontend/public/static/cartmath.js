(function (root, factory) {
  const api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.CartMath = api;
})(typeof self !== 'undefined' ? self : this, function () {
  // Same rules as the server (orders.rs `discount_satang`): the server has the last word, this only
  // lets the till show the net total before the sale is made.
  const MIN_TOTAL_SATANG = 100;

  // Satang a discount takes off `subtotal`, or null when it is not allowed.
  function discountOff(subtotal, d) {
    if (!d) return 0;
    let off;
    if (d.kind === 'percent' && Number.isInteger(d.value) && d.value >= 1 && d.value <= 99) off = Math.floor(subtotal * d.value / 100);
    else if (d.kind === 'amount' && Number.isInteger(d.value) && d.value >= 1) off = d.value;
    else return null;
    if (off < 1 || subtotal - off < MIN_TOTAL_SATANG) return null;
    return off;
  }

  // What the customer pays. A discount that stopped being valid (the cart shrank) counts as none.
  function totals(subtotal, d) {
    const off = discountOff(subtotal, d);
    return { subtotal, discount: off || 0, total: subtotal - (off || 0), valid: off !== null };
  }

  return { discountOff, totals, MIN_TOTAL_SATANG };
});
