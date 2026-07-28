package com.omnisearch.app.ui

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.util.Log
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.annotation.OptIn
import androidx.camera.core.*
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.*
import androidx.compose.material.icons.outlined.ChevronRight
import androidx.compose.material.icons.outlined.FavoriteBorder
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.core.content.ContextCompat
import com.omnisearch.app.ui.theme.FluentTheme
import com.google.mlkit.vision.barcode.BarcodeScanning
import com.google.mlkit.vision.common.InputImage
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

sealed interface PinDialogMode {
    object SetNew : PinDialogMode
    object DisableVerify : PinDialogMode
    object ChangeVerifyCurrent : PinDialogMode
    object ChangeSetNew : PinDialogMode
}

@Composable
fun SettingsScreen(
    syncViewModel: SyncViewModel,
    securityViewModel: SecurityViewModel,
    modifier: Modifier = Modifier,
    openScannerRequest: Boolean = false,
    onOpenScannerHandled: () -> Unit = {}
) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current

    val status by syncViewModel.status.collectAsState()
    val serverAddress by syncViewModel.serverAddress.collectAsState()
    val pairingLink by syncViewModel.pairingLink.collectAsState()
    val pairingMessage by syncViewModel.pairingMessage.collectAsState()
    val error by syncViewModel.error.collectAsState()

    val isLockEnabled by securityViewModel.isPinSet.collectAsState()
    val isBiometricEnabled by securityViewModel.isBiometricEnabled.collectAsState()

    var manualAddressInput by remember { mutableStateOf("") }
    var isScanningQR by remember { mutableStateOf(false) }

    // PIN Management Overlay States
    var pinDialogMode by remember { mutableStateOf<PinDialogMode?>(null) }
    var pinBuffer by remember { mutableStateOf("") }

    var showWidgetsScreen by remember { mutableStateOf(false) }

    // Verify biometric hardware support
    val supportsBiometric = remember {
        val manager = androidx.biometric.BiometricManager.from(context)
        val canAuthenticate = manager.canAuthenticate(androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_STRONG)
        canAuthenticate == androidx.biometric.BiometricManager.BIOMETRIC_SUCCESS
    }

    // Camera Permission Launcher
    val cameraPermissionLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.RequestPermission()
    ) { granted ->
        if (granted) {
            isScanningQR = true
        } else {
            Toast.makeText(context, "Camera permission needed to scan pairing codes", Toast.LENGTH_LONG).show()
        }
    }

    LaunchedEffect(openScannerRequest) {
        if (openScannerRequest) {
            showWidgetsScreen = false
            val hasCam = androidx.core.content.ContextCompat.checkSelfPermission(context, android.Manifest.permission.CAMERA) == android.content.pm.PackageManager.PERMISSION_GRANTED
            if (hasCam) {
                isScanningQR = true
            } else {
                cameraPermissionLauncher.launch(android.Manifest.permission.CAMERA)
            }
            onOpenScannerHandled()
        }
    }

    LaunchedEffect(pairingLink) {
        if (pairingLink.isNotEmpty()) {
            manualAddressInput = pairingLink
        }
    }

    if (showWidgetsScreen) {
        com.omnisearch.app.ui.WidgetsScreen(onBack = { showWidgetsScreen = false })
        return
    }

    Column(
        modifier = modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(FluentTheme.dims.paddingLarge),
        verticalArrangement = Arrangement.spacedBy(16.dp)
    ) {
        // Heading Title Panel
        Text(
            text = "Settings",
            fontSize = 32.sp,
            fontWeight = FontWeight.Bold,
            color = FluentTheme.colors.textColor,
            modifier = Modifier.padding(bottom = 8.dp)
        )

        // CARD 1: PC Sync Connection Card
        Card(
            modifier = Modifier.fillMaxWidth(),
            colors = CardDefaults.cardColors(containerColor = FluentTheme.colors.surfaceBg),
            shape = RoundedCornerShape(FluentTheme.dims.surfaceRadius),
            border = androidx.compose.foundation.BorderStroke(1.dp, FluentTheme.colors.panelBorder)
        ) {
            Column(modifier = Modifier.padding(16.dp)) {
                Text(
                    text = "PC SYNC CONNECTION",
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Bold,
                    color = FluentTheme.colors.textMuted,
                    letterSpacing = 0.5.sp
                )

                Spacer(modifier = Modifier.height(14.dp))

                // Connection status dot and tag
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.fillMaxWidth()
                ) {
                    val statusColor = when (status) {
                        ConnectionStatus.CONNECTED -> FluentTheme.colors.connectedText
                        ConnectionStatus.CONNECTING, ConnectionStatus.AWAITING_APPROVAL -> FluentTheme.colors.accent
                        ConnectionStatus.DISCONNECTED -> FluentTheme.colors.textMuted
                    }
                    val statusText = when (status) {
                        ConnectionStatus.CONNECTED -> "Connected to $serverAddress"
                        ConnectionStatus.AWAITING_APPROVAL -> "Waiting for desktop approval..."
                        ConnectionStatus.CONNECTING -> "Connecting..."
                        ConnectionStatus.DISCONNECTED -> "Disconnected"
                    }

                    Box(
                        modifier = Modifier
                            .size(12.dp)
                            .clip(CircleShape)
                            .background(statusColor)
                    )
                    Spacer(modifier = Modifier.width(8.dp))
                    Text(
                        text = statusText,
                        fontSize = 17.sp,
                        fontWeight = FontWeight.Medium,
                        color = FluentTheme.colors.textColor
                    )
                }

                Spacer(modifier = Modifier.height(16.dp))

                if (status == ConnectionStatus.DISCONNECTED) {
                    OutlinedTextField(
                        value = manualAddressInput,
                        onValueChange = { manualAddressInput = it },
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("manual_address_input"),
                        placeholder = { Text("Paste pairing link or scan QR") },
                        singleLine = true,
                        colors = OutlinedTextFieldDefaults.colors(
                            focusedBorderColor = FluentTheme.colors.accent,
                            unfocusedBorderColor = FluentTheme.colors.panelBorder,
                            focusedTextColor = FluentTheme.colors.textColor,
                            unfocusedTextColor = FluentTheme.colors.textColor
                        )
                    )

                    Spacer(modifier = Modifier.height(12.dp))

                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(8.dp)
                    ) {
                        // Connect Toggle
                        Button(
                            onClick = {
                                if (manualAddressInput.isNotEmpty()) {
                                    syncViewModel.connect(manualAddressInput)
                                }
                            },
                            modifier = Modifier
                                .weight(1f)
                                .testTag("manual_connect_button"),
                            colors = ButtonDefaults.buttonColors(containerColor = FluentTheme.colors.surfaceBg),
                            shape = RoundedCornerShape(FluentTheme.dims.controlRadius),
                            border = androidx.compose.foundation.BorderStroke(1.dp, FluentTheme.colors.panelBorder)
                        ) {
                            Icon(Icons.Default.Link, contentDescription = null, tint = FluentTheme.colors.textColor)
                            Spacer(modifier = Modifier.width(8.dp))
                            Text("Connect", color = FluentTheme.colors.textColor)
                        }

                        // Scan QR trigger
                        Button(
                            onClick = {
                                val hasCam = ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED
                                if (hasCam) {
                                    isScanningQR = true
                                } else {
                                    cameraPermissionLauncher.launch(Manifest.permission.CAMERA)
                                }
                            },
                            modifier = Modifier
                                .weight(1f)
                                .testTag("scan_qr_button"),
                            colors = ButtonDefaults.buttonColors(containerColor = FluentTheme.colors.accent),
                            shape = RoundedCornerShape(FluentTheme.dims.controlRadius)
                        ) {
                            Icon(Icons.Default.QrCodeScanner, contentDescription = null, tint = FluentTheme.colors.onAccent)
                            Spacer(modifier = Modifier.width(8.dp))
                            Text("Scan QR", color = FluentTheme.colors.onAccent)
                        }
                    }
                } else {
                    // Connected/Connecting cancel buttons
                    Button(
                        onClick = { syncViewModel.disconnect() },
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("disconnect_sync_button"),
                        colors = ButtonDefaults.buttonColors(containerColor = FluentTheme.colors.surfaceBg),
                        shape = RoundedCornerShape(FluentTheme.dims.controlRadius),
                        border = androidx.compose.foundation.BorderStroke(1.dp, FluentTheme.colors.panelBorder)
                    ) {
                        Icon(
                            imageVector = Icons.Default.Cancel,
                            contentDescription = null,
                            tint = FluentTheme.colors.dangerText
                        )
                        Spacer(modifier = Modifier.width(8.dp))
                        Text(
                            text = if (status == ConnectionStatus.AWAITING_APPROVAL) "Cancel Pairing" else "Disconnect Sync Server",
                            color = FluentTheme.colors.dangerText
                        )
                    }
                }

                // Inline Errors / Pair notifications
                if (pairingMessage.isNotEmpty()) {
                    Spacer(modifier = Modifier.height(10.dp))
                    Text(pairingMessage, fontSize = 13.sp, color = FluentTheme.colors.textMuted)
                }
                if (error.isNotEmpty()) {
                    Spacer(modifier = Modifier.height(10.dp))
                    Text(error, fontSize = 13.sp, color = FluentTheme.colors.dangerText)
                }
            }
        }

        // CARD 2: Security settings (gating locks, credentials)
        Card(
            modifier = Modifier.fillMaxWidth(),
            colors = CardDefaults.cardColors(containerColor = FluentTheme.colors.surfaceBg),
            shape = RoundedCornerShape(FluentTheme.dims.surfaceRadius),
            border = androidx.compose.foundation.BorderStroke(1.dp, FluentTheme.colors.panelBorder)
        ) {
            Column(modifier = Modifier.padding(16.dp)) {
                Text(
                    text = "SECURITY CONTROLS",
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Bold,
                    color = FluentTheme.colors.textMuted,
                    letterSpacing = 0.5.sp
                )

                Spacer(modifier = Modifier.height(12.dp))

                // Toggle 1: PIN App Lock
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(vertical = 12.dp),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically
                ) {
                    Column {
                        Text("Enable App Lock", fontSize = 16.sp, fontWeight = FontWeight.Medium, color = FluentTheme.colors.textColor)
                        Text("Locks data behind 4-digit PIN gate", fontSize = 12.sp, color = FluentTheme.colors.textMuted)
                    }
                    Switch(
                        checked = isLockEnabled,
                        onCheckedChange = { active ->
                            pinBuffer = ""
                            if (active) {
                                pinDialogMode = PinDialogMode.SetNew
                            } else {
                                pinDialogMode = PinDialogMode.DisableVerify
                            }
                        },
                        colors = SwitchDefaults.colors(
                            checkedThumbColor = Color.White,
                            checkedTrackColor = FluentTheme.colors.accent,
                            uncheckedThumbColor = FluentTheme.colors.textMuted,
                            uncheckedTrackColor = FluentTheme.colors.surfaceBg,
                            uncheckedBorderColor = FluentTheme.colors.panelBorder
                        )
                    )
                }

                // Change PIN entry
                if (isLockEnabled) {
                    Divider(color = FluentTheme.colors.panelBorder, thickness = 1.dp)
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .clickable {
                                pinBuffer = ""
                                pinDialogMode = PinDialogMode.ChangeVerifyCurrent
                            }
                            .padding(vertical = 14.dp),
                        horizontalArrangement = Arrangement.SpaceBetween,
                        verticalAlignment = Alignment.CenterVertically
                    ) {
                        Text("Change PIN code", fontSize = 16.sp, fontWeight = FontWeight.Medium, color = FluentTheme.colors.textColor)
                        Icon(
                            imageVector = Icons.Default.LockReset,
                            contentDescription = "Change security pin",
                            tint = FluentTheme.colors.textMuted
                        )
                    }
                }

                // Toggle 2: Biometric check
                if (isLockEnabled && supportsBiometric) {
                    Divider(color = FluentTheme.colors.panelBorder, thickness = 1.dp)
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(vertical = 12.dp),
                        horizontalArrangement = Arrangement.SpaceBetween,
                        verticalAlignment = Alignment.CenterVertically
                    ) {
                        Column {
                            Text("Biometric Unlock", fontSize = 16.sp, fontWeight = FontWeight.Medium, color = FluentTheme.colors.textColor)
                            Text("Use FaceID/Fingerprint bypass", fontSize = 12.sp, color = FluentTheme.colors.textMuted)
                        }
                        Switch(
                            checked = isBiometricEnabled,
                            onCheckedChange = { securityViewModel.toggleBiometric(it) },
                            colors = SwitchDefaults.colors(
                                checkedThumbColor = Color.White,
                                checkedTrackColor = FluentTheme.colors.accent,
                                uncheckedThumbColor = FluentTheme.colors.textMuted,
                                uncheckedTrackColor = FluentTheme.colors.surfaceBg,
                                uncheckedBorderColor = FluentTheme.colors.panelBorder
                            )
                        )
                    }
                }
            }
        }

        // CARD 3: Customization
        androidx.compose.material3.Card(
            modifier = Modifier.fillMaxWidth(),
            colors = androidx.compose.material3.CardDefaults.cardColors(containerColor = FluentTheme.colors.surfaceBg),
            shape = androidx.compose.foundation.shape.RoundedCornerShape(FluentTheme.dims.surfaceRadius),
            border = androidx.compose.foundation.BorderStroke(1.dp, FluentTheme.colors.panelBorder)
        ) {
            Column(modifier = Modifier.padding(16.dp)) {
                Text(
                    text = "CUSTOMIZATION",
                    fontSize = 11.sp,
                    fontWeight = androidx.compose.ui.text.font.FontWeight.Bold,
                    color = FluentTheme.colors.textMuted,
                    letterSpacing = 0.5.sp
                )
                Spacer(modifier = Modifier.height(12.dp))
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable { showWidgetsScreen = true }
                        .padding(vertical = 14.dp),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically
                ) {
                    Column {
                        Text("Home Screen Widgets", fontSize = 16.sp, fontWeight = androidx.compose.ui.text.font.FontWeight.Medium, color = FluentTheme.colors.textColor)
                        Text("Add or customize OmniSearch widgets", fontSize = 12.sp, color = FluentTheme.colors.textMuted)
                    }
                    Icon(
                        imageVector = Icons.Default.Apps,
                        contentDescription = "Widgets",
                        tint = FluentTheme.colors.textMuted
                    )
                }
            }
        }
        Spacer(modifier = Modifier.height(24.dp))

        // CARD 4: About developers credentials
        Card(
            modifier = Modifier.fillMaxWidth(),
            colors = CardDefaults.cardColors(containerColor = FluentTheme.colors.surfaceBg),
            shape = RoundedCornerShape(FluentTheme.dims.surfaceRadius),
            border = androidx.compose.foundation.BorderStroke(1.dp, FluentTheme.colors.panelBorder)
        ) {
            Column(modifier = Modifier.padding(16.dp)) {
                Text(
                    text = "ABOUT OMNISEARCH",
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Bold,
                    color = FluentTheme.colors.textMuted,
                    letterSpacing = 0.5.sp
                )

                Spacer(modifier = Modifier.height(14.dp))

                val packageVersion = remember {
                    try {
                        val packageInfo = context.packageManager.getPackageInfo(context.packageName, 0)
                        packageInfo.versionName ?: "1.0"
                    } catch (e: Exception) {
                        "1.0"
                    }
                }
                Text("OmniSearch v$packageVersion", fontSize = 17.sp, fontWeight = FontWeight.Bold, color = FluentTheme.colors.textColor)
                Spacer(modifier = Modifier.height(4.dp))
                Text(
                    text = "Secure local sync engine built to index, query, search, and manage files on your PC over private WiFi network. All computations on-device.",
                    fontSize = 13.sp,
                    lineHeight = 20.sp,
                    color = FluentTheme.colors.textColor
                )

                Spacer(modifier = Modifier.height(12.dp))
                Divider(color = FluentTheme.colors.panelBorder, thickness = 1.dp)
                Spacer(modifier = Modifier.height(12.dp))

                Text(
                    text = "Developed by Eyuel Engida",
                    fontSize = 13.sp,
                    color = FluentTheme.colors.textMuted
                )

                Spacer(modifier = Modifier.height(14.dp))

                // Social Icon links row
                Row(
                    horizontalArrangement = Arrangement.spacedBy(20.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.fillMaxWidth()
                ) {
                    SocialButton(label = "GitHub", icon = Icons.Default.Code, url = "https://github.com/Eul45", context = context)
                    SocialButton(label = "LinkedIn", icon = Icons.Default.AccountCircle, url = "https://www.linkedin.com/in/eyuel-engida-77155a317", context = context)
                    SocialButton(label = "Telegram", icon = Icons.Default.Send, url = "https://t.me/Eul_zzz", context = context)
                }

                Spacer(modifier = Modifier.height(14.dp))

                // OmniSearch GitHub Page link
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(FluentTheme.dims.controlRadius))
                        .background(FluentTheme.colors.surfaceBg)
                        .border(1.dp, FluentTheme.colors.panelBorder, RoundedCornerShape(FluentTheme.dims.controlRadius))
                        .clickable {
                            try {
                                val intent = Intent(Intent.ACTION_VIEW, Uri.parse("https://github.com/Eul45/omni-search"))
                                context.startActivity(intent)
                            } catch (_: Exception) {}
                        }
                        .padding(horizontal = 14.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically
                ) {
                    Icon(Icons.Default.Code, contentDescription = "OmniSearch GitHub Page", tint = FluentTheme.colors.accent, modifier = Modifier.size(20.dp))
                    Spacer(modifier = Modifier.width(10.dp))
                    Text(
                        text = "OmniSearch GitHub Page",
                        fontSize = 14.sp,
                        fontWeight = FontWeight.Medium,
                        color = FluentTheme.colors.textColor,
                        modifier = Modifier.weight(1f)
                    )
                    Icon(Icons.Outlined.ChevronRight, contentDescription = null, tint = FluentTheme.colors.textMuted, modifier = Modifier.size(18.dp))
                }

                Spacer(modifier = Modifier.height(16.dp))

                // Primary Main Button: GitHub Sponsor (Sponsor Pink Theme)
                Button(
                    onClick = {
                        val intent = Intent(Intent.ACTION_VIEW, Uri.parse("https://github.com/sponsors/Eul45"))
                        context.startActivity(intent)
                    },
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(48.dp),
                    colors = ButtonDefaults.buttonColors(containerColor = Color(0xFFEA4AAA)),
                    shape = RoundedCornerShape(FluentTheme.dims.controlRadius)
                ) {
                    Icon(
                        imageVector = Icons.Outlined.FavoriteBorder,
                        contentDescription = null,
                        tint = Color.White,
                        modifier = Modifier.size(20.dp)
                    )
                    Spacer(modifier = Modifier.width(8.dp))
                    Text(
                        text = "GitHub Sponsor",
                        color = Color.White,
                        fontSize = 15.sp,
                        fontWeight = FontWeight.Bold
                    )
                }

                Spacer(modifier = Modifier.height(10.dp))

                // Secondary Small Button: Buy me a coffee
                OutlinedButton(
                    onClick = {
                        val intent = Intent(Intent.ACTION_VIEW, Uri.parse("http://buymeacoffee.com/eyuelengida"))
                        context.startActivity(intent)
                    },
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(40.dp),
                    shape = RoundedCornerShape(FluentTheme.dims.controlRadius),
                    border = androidx.compose.foundation.BorderStroke(1.dp, Color(0xFFFFDD00).copy(alpha = 0.6f)),
                    colors = ButtonDefaults.outlinedButtonColors(
                        containerColor = FluentTheme.colors.surfaceBg,
                        contentColor = Color(0xFFFFDD00)
                    )
                ) {
                    Icon(
                        imageVector = Icons.Default.Coffee,
                        contentDescription = null,
                        tint = Color(0xFFFFDD00),
                        modifier = Modifier.size(15.dp)
                    )
                    Spacer(modifier = Modifier.width(6.dp))
                    Text(
                        text = "Buy me a coffee",
                        color = Color(0xFFFFDD00),
                        fontSize = 13.sp,
                        fontWeight = FontWeight.Medium
                    )
                }
            }
        }

        Spacer(modifier = Modifier.height(60.dp))
    }

    // Modal QR Scanning Overly
    if (isScanningQR) {
        Dialog(
            onDismissRequest = { isScanningQR = false },
            properties = DialogProperties(usePlatformDefaultWidth = false)
        ) {
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .background(Color.Black)
            ) {
                CameraQRScannerView(
                    onQRCodeScanned = { scannedUrl ->
                        isScanningQR = false
                        if (scannedUrl.startsWith("omnisearch://")) {
                            manualAddressInput = scannedUrl
                            syncViewModel.connect(scannedUrl)
                        } else {
                            Toast.makeText(context, "Invalid QR code formats", Toast.LENGTH_SHORT).show()
                        }
                    },
                    onClose = { isScanningQR = false }
                )
            }
        }
    }

    // Modal Security PIN Keypad Dialog Setup
    if (pinDialogMode != null) {
        val currentMode = pinDialogMode!!
        val dialogTitle = when (currentMode) {
            PinDialogMode.SetNew -> "Enter New PIN"
            PinDialogMode.ChangeSetNew -> "Enter New PIN"
            PinDialogMode.DisableVerify -> "Verify Current PIN"
            PinDialogMode.ChangeVerifyCurrent -> "Verify Current PIN"
        }
        val dialogSub = when (currentMode) {
            PinDialogMode.SetNew, PinDialogMode.ChangeSetNew -> "Choose a 4-digit security code"
            else -> "Input active code to authorize edits"
        }

        Dialog(
            onDismissRequest = { pinDialogMode = null }
        ) {
            Card(
                colors = CardDefaults.cardColors(containerColor = FluentTheme.colors.pageBg),
                modifier = Modifier
                    .fillMaxWidth(0.95f)
                    .border(1.dp, FluentTheme.colors.panelBorder, RoundedCornerShape(FluentTheme.dims.surfaceRadius)),
                shape = RoundedCornerShape(FluentTheme.dims.surfaceRadius)
            ) {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(20.dp),
                    horizontalAlignment = Alignment.CenterHorizontally
                ) {
                    Text(dialogTitle, fontSize = 20.sp, fontWeight = FontWeight.Bold, color = FluentTheme.colors.textColor)
                    Spacer(modifier = Modifier.height(4.dp))
                    Text(dialogSub, fontSize = 13.sp, color = FluentTheme.colors.textMuted)

                    Spacer(modifier = Modifier.height(24.dp))

                    // Indicator Dots Row
                    Row(
                        horizontalArrangement = Arrangement.spacedBy(16.dp),
                        verticalAlignment = Alignment.CenterVertically
                    ) {
                        for (i in 0..3) {
                            val active = pinBuffer.length > i
                            Box(
                                modifier = Modifier
                                    .size(16.dp)
                                    .clip(CircleShape)
                                    .border(2.dp, FluentTheme.colors.accent, CircleShape)
                                    .background(if (active) FluentTheme.colors.accent else Color.Transparent)
                            )
                        }
                    }

                    Spacer(modifier = Modifier.height(32.dp))

                    val rows = listOf(
                        listOf("1", "2", "3"),
                        listOf("4", "5", "6"),
                        listOf("7", "8", "9"),
                        listOf("cancel", "0", "⌫")
                    )
                    Column(
                        verticalArrangement = Arrangement.spacedBy(16.dp),
                        horizontalAlignment = Alignment.CenterHorizontally
                    ) {
                        rows.forEach { rowKeys ->
                            Row(
                                horizontalArrangement = Arrangement.spacedBy(24.dp),
                                verticalAlignment = Alignment.CenterVertically
                            ) {
                                rowKeys.forEach { key ->
                                    when (key) {
                                        "cancel" -> {
                                            Box(
                                                modifier = Modifier
                                                    .size(64.dp)
                                                    .clip(CircleShape)
                                                    .background(FluentTheme.colors.surfaceBg)
                                                    .clickable { pinDialogMode = null },
                                                contentAlignment = Alignment.Center
                                            ) {
                                                Icon(
                                                    imageVector = Icons.Default.Close,
                                                    contentDescription = "Cancel",
                                                    tint = FluentTheme.colors.textColor,
                                                    modifier = Modifier.size(22.dp)
                                                )
                                            }
                                        }
                                        "⌫" -> {
                                            Box(
                                                modifier = Modifier
                                                    .size(64.dp)
                                                    .clip(CircleShape)
                                                    .background(FluentTheme.colors.surfaceBg)
                                                    .clickable {
                                                        if (pinBuffer.isNotEmpty()) pinBuffer = pinBuffer.dropLast(1)
                                                    },
                                                contentAlignment = Alignment.Center
                                            ) {
                                                Icon(
                                                    imageVector = Icons.Default.Backspace,
                                                    contentDescription = "Delete",
                                                    tint = FluentTheme.colors.textColor,
                                                    modifier = Modifier.size(20.dp)
                                                )
                                            }
                                        }
                                        else -> {
                                            Box(
                                                modifier = Modifier
                                                    .size(64.dp)
                                                    .clip(CircleShape)
                                                    .background(FluentTheme.colors.surfaceBg)
                                                    .border(1.dp, FluentTheme.colors.panelBorder, CircleShape)
                                                    .clickable {
                                                        if (pinBuffer.length < 4) {
                                                            pinBuffer += key
                                                            if (pinBuffer.length == 4) {
                                                                val savedPIN = securityViewModel.savedPin
                                                                when (currentMode) {
                                                                    PinDialogMode.SetNew -> {
                                                                        securityViewModel.setAppLockPin(pinBuffer)
                                                                        pinDialogMode = null
                                                                        Toast.makeText(context, "App lock PIN enabled!", Toast.LENGTH_SHORT).show()
                                                                    }
                                                                    PinDialogMode.DisableVerify -> {
                                                                        if (pinBuffer == savedPIN) {
                                                                            securityViewModel.disableAppLock()
                                                                            pinDialogMode = null
                                                                            Toast.makeText(context, "App lock PIN disabled", Toast.LENGTH_SHORT).show()
                                                                        } else {
                                                                            Toast.makeText(context, "Incorrect verification code", Toast.LENGTH_SHORT).show()
                                                                            pinBuffer = ""
                                                                        }
                                                                    }
                                                                    PinDialogMode.ChangeVerifyCurrent -> {
                                                                        if (pinBuffer == savedPIN) {
                                                                            pinBuffer = ""
                                                                            pinDialogMode = PinDialogMode.ChangeSetNew
                                                                        } else {
                                                                            Toast.makeText(context, "Incorrect verification code", Toast.LENGTH_SHORT).show()
                                                                            pinBuffer = ""
                                                                        }
                                                                    }
                                                                    PinDialogMode.ChangeSetNew -> {
                                                                        securityViewModel.setAppLockPin(pinBuffer)
                                                                        pinDialogMode = null
                                                                        Toast.makeText(context, "PIN code edited successfully", Toast.LENGTH_SHORT).show()
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    },
                                                contentAlignment = Alignment.Center
                                            ) {
                                                Text(key, fontSize = 24.sp, fontWeight = FontWeight.Normal, color = FluentTheme.colors.textColor)
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
fun SocialButton(
    label: String,
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    url: String,
    context: Context
) {
    Row(
        modifier = Modifier
            .clip(RoundedCornerShape(6.dp))
            .background(FluentTheme.colors.surfaceBg)
            .border(1.dp, FluentTheme.colors.panelBorder, RoundedCornerShape(6.dp))
            .clickable {
                try {
                    val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url))
                    context.startActivity(intent)
                } catch (_: Exception) {}
            }
            .padding(horizontal = 10.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically
    ) {
        Icon(icon, contentDescription = label, tint = FluentTheme.colors.accent, modifier = Modifier.size(16.dp))
        Spacer(modifier = Modifier.width(4.dp))
        Text(label, fontSize = 12.sp, fontWeight = FontWeight.Medium, color = FluentTheme.colors.textColor)
    }
}

@OptIn(ExperimentalGetImage::class)
@Composable
fun CameraQRScannerView(
    onQRCodeScanned: (String) -> Unit,
    onClose: () -> Unit
) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current

    val cameraProviderFuture = remember { ProcessCameraProvider.getInstance(context) }
    val cameraExecutor: ExecutorService = remember { Executors.newSingleThreadExecutor() }

    Box(modifier = Modifier.fillMaxSize()) {
        // Camera Stream
        AndroidView(
            modifier = Modifier.fillMaxSize(),
            factory = { ctx ->
                val previewView = PreviewView(ctx)
                cameraProviderFuture.addListener({
                    val cameraProvider = cameraProviderFuture.get()
                    val preview = Preview.Builder().build().also {
                        it.setSurfaceProvider(previewView.surfaceProvider)
                    }

                    val imageAnalysis = ImageAnalysis.Builder()
                        .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                        .build()
                        .also { analysis ->
                            analysis.setAnalyzer(cameraExecutor) { imageProxy ->
                                val mediaImage = imageProxy.image
                                if (mediaImage != null) {
                                    val image = InputImage.fromMediaImage(mediaImage, imageProxy.imageInfo.rotationDegrees)
                                    val scanner = BarcodeScanning.getClient()
                                    scanner.process(image)
                                        .addOnSuccessListener { barcodes ->
                                            for (barcode in barcodes) {
                                                val rawValue = barcode.rawValue
                                                if (rawValue != null) {
                                                    onQRCodeScanned(rawValue)
                                                    break
                                                }
                                            }
                                        }
                                        .addOnFailureListener {
                                            Log.e("Scanner", "Scanning failed", it)
                                        }
                                        .addOnCompleteListener {
                                            imageProxy.close()
                                        }
                                } else {
                                    imageProxy.close()
                                }
                            }
                        }

                    val cameraSelector = CameraSelector.DEFAULT_BACK_CAMERA
                    try {
                        cameraProvider.unbindAll()
                        cameraProvider.bindToLifecycle(
                            lifecycleOwner,
                            cameraSelector,
                            preview,
                            imageAnalysis
                        )
                    } catch (e: Exception) {
                        Log.e("Scanner", "Camera pairing failed", e)
                    }
                }, ContextCompat.getMainExecutor(ctx))
                previewView
            }
        )

        // Semi-transparent target scanner bounds
        Box(
            modifier = Modifier
                .fillMaxSize()
                .background(Color.Black.copy(alpha = 0.5f))
        ) {
            Column(
                modifier = Modifier.fillMaxSize(),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center
            ) {
                Text(
                    "Scan OmniSearch QR Code",
                    fontSize = 20.sp,
                    color = Color.White,
                    fontWeight = FontWeight.Bold
                )
                Spacer(modifier = Modifier.height(16.dp))
                // scanning box cutout reference
                Box(
                    modifier = Modifier
                        .size(240.dp)
                        .border(2.dp, FluentTheme.colors.accent, RoundedCornerShape(12.dp))
                )
                Spacer(modifier = Modifier.height(32.dp))
                Button(
                    onClick = onClose,
                    colors = ButtonDefaults.buttonColors(containerColor = FluentTheme.colors.pageBg),
                    shape = RoundedCornerShape(FluentTheme.dims.controlRadius)
                ) {
                    Text("Cancel scanning", color = FluentTheme.colors.textColor)
                }
            }
        }
    }
}
