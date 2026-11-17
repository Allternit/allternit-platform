package com.allternit.app

import com.google.androidbrowserhelper.playbilling.digitalgoods.DigitalGoodsRequestHandler
import com.google.androidbrowserhelper.trusted.DelegationService

/**
 * The Google Play build's Trusted Web Activity service: androidbrowserhelper's
 * DelegationService (notification delegation, as in the phonelink build) plus the
 * Digital Goods API handler, so m.allternit.com can call `getDigitalGoodsService(
 * "https://play.google.com/billing")` for prices, purchases and acknowledgements
 * through Google Play Billing (store purchases D15). Purchases themselves run through
 * PaymentRequest, which Chrome hands to the library's PaymentActivity (play manifest).
 *
 * The play manifest removes the stock DelegationService that src/main declares, so
 * Chrome finds exactly one TRUSTED_WEB_ACTIVITY_SERVICE in this package.
 */
class PlayDelegationService : DelegationService() {
    override fun onCreate() {
        super.onCreate()
        registerExtraCommandHandler(DigitalGoodsRequestHandler(applicationContext))
    }
}
